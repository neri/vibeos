//! ネットワーク（DESIGN.md §12）
//!
//! TCP/IP の中身は `vibe-net`（ハードウェアに依存しない）。ここではそれをカーネルにつなぐ:
//! - 起動時に BOOT.CFG を読み、カード（drivers/nic: NE2000 か RTL8139）を探してスタックを作る
//! - **net スレッド**: カードから受け取ったフレームをスタックに渡し、時間を進め（再送など）、
//!   送るフレームをカードに渡す。割り込みハンドラはこのスレッドを起こすだけ
//! - ソケット: スタックの操作は待たないので、ここで `WaitQueue` を使って待つ（時間切れ・Ctrl+C 付き）。
//!   ソケットはファイルと同じ番号の表に入る（fs::add_socket）。最後の参照が無くなると閉じる
//!
//! ロックの順番: STACK → CARD（各ドライバ）。STACK を持ったまま待たない。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use vibe_abi::err;
use vibe_net::config::{self, Device, NetSettings};
use vibe_net::{IfConfig, Ipv4, Mac, NetError, Readiness, SocketId, Stack, Stats};

use crate::drivers::{nic, pit};
use crate::sync::IrqMutex;
use crate::task::wait::WaitQueue;
use crate::task::{self, sched};

static STACK: IrqMutex<Option<Stack>> = IrqMutex::new(None);
static SETTINGS: IrqMutex<Option<NetSettings>> = IrqMutex::new(None);
/// net スレッドが待つ（カードの割り込み・送るものができた）
static NET_WAIT: WaitQueue = WaitQueue::new();
/// ソケットを使う側が待つ（net スレッドが 1 回りするたびに起こす）
static SOCK_WAIT: WaitQueue = WaitQueue::new();
/// 送るものができた（ソケットの操作の後）
static KICK: AtomicBool = AtomicBool::new(false);

/// 送り終わりを待つ上限（刻み）
const TX_TIMEOUT_TICKS: u32 = 10;

fn now_ms() -> u64 {
    pit::ticks() as u64 * 1000 / pit::TICK_HZ as u64
}

fn ms_to_ticks(ms: u32) -> u32 {
    ((ms as u64 * pit::TICK_HZ as u64).div_ceil(1000)).min(u32::MAX as u64 / 2) as u32
}

/// スタックを使う（ネットワークが無ければ ENODEV）
fn with_stack<R>(f: impl FnOnce(&mut Stack) -> R) -> Result<R, i32> {
    let mut g = STACK.lock();
    let s = g.as_mut().ok_or(err::ENODEV)?;
    s.set_now(now_ms());
    Ok(f(s))
}

/// net スレッドを起こす（割り込みハンドラから呼んでよい）
pub fn wake_net_thread() {
    NET_WAIT.notify_all();
}

/// 送るものができたことを net スレッドに知らせる
fn kick() {
    KICK.store(true, Ordering::Release);
    NET_WAIT.notify_all();
}

/// ソケットで待っているタスクを起こす（Ctrl+C を伝えるため。割り込みハンドラから呼んでよい）
pub fn wake_sockets() {
    SOCK_WAIT.notify_all();
}

pub fn errno(e: NetError) -> i32 {
    match e {
        NetError::NotConfigured => err::ENETDOWN,
        NetError::NoRoute => err::ENETUNREACH,
        NetError::HostUnreachable => err::EHOSTUNREACH,
        NetError::AddrInUse => err::EADDRINUSE,
        NetError::BadSocket => err::EBADF,
        NetError::NotConnected => err::ENOTCONN,
        NetError::ConnRefused => err::ECONNREFUSED,
        NetError::ConnReset => err::ECONNRESET,
        NetError::TimedOut => err::ETIMEDOUT,
        NetError::WouldBlock => err::EAGAIN,
        NetError::TooBig => err::EMSGSIZE,
        NetError::InvalidArgument => err::EINVAL,
    }
}

// --- 起動 ---------------------------------------------------------------------

/// BOOT.CFG を読んでカードを探し、あれば net スレッドを動かす（ファイルシステムと割り込みの準備の後）
pub fn init() {
    let text = crate::fs::read_file("/BOOT.CFG").unwrap_or_default();
    let text = core::str::from_utf8(&text).unwrap_or("");
    let settings = config::parse(text, |bad| klog!("net: cannot read BOOT.CFG line: {}", bad.0));
    *SETTINGS.lock() = Some(settings);
    if settings.device == Device::None {
        klog!("net: disabled (net=none)");
        return;
    }
    let Some(card) = nic::probe(settings.device) else {
        match settings.device {
            Device::Ne2000 { io, .. } => klog!("net: no NE2000 at {:#x}", io),
            _ => klog!("net: no network card (PCI, or NE2000 at 0x300)"),
        }
        return;
    };
    klog!("net: {} at {:#x} irq {}, MAC {}", card.name, card.io, card.irq, card.mac);
    let mac = card.mac;
    let m = mac.0;
    let seed = u32::from_le_bytes([m[2], m[3], m[4], m[5]]) ^ crate::drivers::rtc::now_secs() ^ pit::ticks();
    let mut stack = Stack::new(mac, seed);
    stack.set_overrides(settings.gateway, settings.dns);
    // カードの受信バッファは小さい（NE2000 は 1514 バイトのフレームが 9 つ）。net スレッドが取り出すまでに
    // 相手が窓いっぱいに送ってきてもあふれないよう、TCP の窓を小さくする
    stack.set_recv_window(nic::recv_window());
    match settings.static_ip {
        Some((addr, prefix)) => stack.set_static(IfConfig { addr, prefix, gateway: settings.gateway, dns: settings.dns }),
        None => stack.start_dhcp(),
    }
    *STACK.lock() = Some(stack);
    task::spawn("net", net_thread);
}

fn net_thread() {
    loop {
        // 受け取る
        let now = now_ms();
        nic::receive_all(|frame| {
            if let Some(s) = STACK.lock().as_mut() {
                s.receive(frame, now);
            }
        });
        // 時間を進める
        let event = {
            let mut g = STACK.lock();
            let s = g.as_mut().unwrap();
            s.poll(now_ms());
            s.take_config_event().map(|c| (c, s.dhcp_enabled()))
        };
        if let Some((c, dhcp)) = event {
            klog!(
                "net: address {}/{} gateway {} dns {} ({})",
                c.addr,
                c.prefix,
                OptIp(c.gateway),
                OptIp(c.dns),
                if dhcp { "dhcp" } else { "static" }
            );
        }
        // 送る
        loop {
            let frame = STACK.lock().as_mut().and_then(|s| s.transmit());
            let Some(frame) = frame else { break };
            wait_tx_idle();
            nic::send(&frame);
        }
        SOCK_WAIT.notify_all();

        // 次の仕事まで待つ（割り込み・ソケットの操作・時間切れ。割り込みを取りこぼしても 1 秒ごとに見る）
        let deadline = STACK.lock().as_ref().and_then(|s| s.next_deadline());
        let start = pit::ticks();
        let limit = match deadline {
            Some(d) => ms_to_ticks(d.saturating_sub(now_ms()).min(1000) as u32),
            None => pit::TICK_HZ,
        };
        WaitQueue::wait_until_any(&[&NET_WAIT, pit::tick_queue()], || {
            KICK.swap(false, Ordering::AcqRel) || nic::has_events() || pit::ticks().wrapping_sub(start) >= limit
        });
    }
}

/// 前の送信が終わるのを待つ（終わらなければカードを初期化し直す）
fn wait_tx_idle() {
    let start = pit::ticks();
    WaitQueue::wait_until_any(&[&NET_WAIT, pit::tick_queue()], || {
        nic::tx_idle() || pit::ticks().wrapping_sub(start) > TX_TIMEOUT_TICKS
    });
    if !nic::tx_idle() {
        nic::tx_timeout();
    }
}

/// Option<Ipv4> の表示（無ければ "-"）
pub struct OptIp(pub Option<Ipv4>);

impl core::fmt::Display for OptIp {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self.0 {
            Some(ip) => core::fmt::Display::fmt(&ip, f),
            None => f.pad("-"),
        }
    }
}

// --- 情報 ---------------------------------------------------------------------

pub struct Info {
    pub card: &'static str,
    pub mac: Mac,
    pub io: u16,
    pub irq: u8,
    pub config: Option<IfConfig>,
    pub dhcp: bool,
    pub stats: Stats,
    pub settings: NetSettings,
}

/// カードとアドレスの情報（ネットワークが無ければ None）
pub fn info() -> Option<Info> {
    let settings = (*SETTINGS.lock())?;
    let card = nic::info()?;
    let g = STACK.lock();
    let s = g.as_ref()?;
    Some(Info {
        card: card.name,
        mac: s.mac(),
        io: card.io,
        irq: card.irq,
        config: s.config(),
        dhcp: s.dhcp_enabled(),
        stats: s.stats,
        settings,
    })
}

/// 今のアドレス（ステータスバー用）
pub fn address() -> Option<Ipv4> {
    STACK.lock().as_ref().and_then(|s| s.config()).map(|c| c.addr)
}

/// ソケットの一覧（net コマンド用）
pub fn socket_list() -> Vec<vibe_net::stack::SocketInfo> {
    with_stack(|s| s.socket_list()).unwrap_or_default()
}

/// ARP の表（net コマンド用）
pub fn arp_entries() -> Vec<(Ipv4, Mac)> {
    with_stack(|s| s.arp_entries()).unwrap_or_default()
}

/// ARP で MAC アドレスを引く（最大 timeout_ms 待つ）
pub fn arp_resolve(ip: Ipv4, timeout_ms: u32) -> Result<Mac, i32> {
    with_stack(|s| s.arp_query(ip)).and_then(|r| r.map_err(errno))?;
    kick();
    let start = pit::ticks();
    let limit = ms_to_ticks(timeout_ms);
    let mut found = None;
    WaitQueue::wait_until_any(&[&SOCK_WAIT, pit::tick_queue()], || {
        let g = STACK.lock();
        let Some(s) = g.as_ref() else { return true };
        found = s.arp_lookup(ip);
        found.is_some() || !s.arp_pending(ip) || pit::ticks().wrapping_sub(start) >= limit
    });
    found.ok_or(err::EHOSTUNREACH)
}

// --- ソケット ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Stream,
    Dgram,
    Icmp,
}

/// ファイルの表に入るソケット。最後の参照が無くなると閉じる
pub struct Socket {
    kind: Kind,
    /// スタックのソケット（TCP は connect / listen まで、UDP は最初に使うまで無い）
    id: IrqMutex<Option<SocketId>>,
    /// bind したポート（0 = まだ）
    port: AtomicU32,
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(id) = self.id.lock().take() {
            let _ = with_stack(|s| s.close(id));
            kick();
        }
    }
}

impl Socket {
    pub fn kind(&self) -> Kind {
        self.kind
    }

    fn id(&self) -> Option<SocketId> {
        *self.id.lock()
    }
}

/// 待ちの結果（条件が成り立った・時間切れ・Ctrl+C・ネットワークが無い）
fn wait_for(id: SocketId, timeout_ticks: Option<u32>, mut cond: impl FnMut(&Readiness) -> bool) -> Result<(), i32> {
    let me = sched::current_id();
    let start = pit::ticks();
    let mut result = Ok(());
    let mut check = || {
        let r = match STACK.lock().as_ref() {
            Some(s) => s.readiness(id),
            None => {
                result = Err(err::ENODEV);
                return true;
            }
        };
        if cond(&r) {
            return true;
        }
        if crate::process::kill_pending(me) {
            result = Err(err::EINTR);
            return true;
        }
        if timeout_ticks.is_some_and(|t| pit::ticks().wrapping_sub(start) >= t) {
            result = Err(err::ETIMEDOUT);
            return true;
        }
        false
    };
    if timeout_ticks.is_some() {
        WaitQueue::wait_until_any(&[&SOCK_WAIT, pit::tick_queue()], &mut check);
    } else {
        SOCK_WAIT.wait_until(&mut check);
    }
    result
}

/// timeout_ms（u32::MAX はいつまでも）を刻みに
fn timeout_of(ms: u32) -> Option<u32> {
    (ms != u32::MAX).then(|| ms_to_ticks(ms))
}

pub fn socket(kind: u32) -> Result<Arc<Socket>, i32> {
    let kind = match kind {
        vibe_abi::SOCK_STREAM => Kind::Stream,
        vibe_abi::SOCK_DGRAM => Kind::Dgram,
        vibe_abi::SOCK_ICMP => Kind::Icmp,
        _ => return Err(err::EINVAL),
    };
    let id = match kind {
        Kind::Icmp => Some(with_stack(|s| s.icmp_open())?.map_err(errno)?),
        _ => {
            with_stack(|_| ())?;
            None
        }
    };
    Ok(Arc::new(Socket { kind, id: IrqMutex::new(id), port: AtomicU32::new(0) }))
}

pub fn bind(sock: &Socket, port: u16) -> Result<(), i32> {
    if sock.id().is_some() || sock.port.load(Ordering::Relaxed) != 0 {
        return Err(err::EINVAL);
    }
    match sock.kind {
        Kind::Dgram => {
            let id = with_stack(|s| s.udp_open(port))?.map_err(errno)?;
            *sock.id.lock() = Some(id);
        }
        Kind::Stream => {}
        Kind::Icmp => return Err(err::EINVAL),
    }
    sock.port.store(port as u32, Ordering::Relaxed);
    Ok(())
}

/// UDP のソケットを（まだなら空いているポートで）作る
fn udp_id(sock: &Socket) -> Result<SocketId, i32> {
    if let Some(id) = sock.id() {
        return Ok(id);
    }
    let id = with_stack(|s| s.udp_open(0))?.map_err(errno)?;
    *sock.id.lock() = Some(id);
    Ok(id)
}

/// TCP でつなぐ（つながるか失敗するまで待つ）
pub fn connect(sock: &Socket, addr: Ipv4, port: u16) -> Result<(), i32> {
    if sock.kind != Kind::Stream || sock.id().is_some() {
        return Err(err::EINVAL);
    }
    let id = with_stack(|s| s.tcp_connect(addr, port))?.map_err(errno)?;
    *sock.id.lock() = Some(id);
    kick();
    wait_for(id, None, |r| r.connected)?;
    match with_stack(|s| s.tcp_error(id))? {
        Some(e) => Err(errno(e)),
        None => Ok(()),
    }
}

pub fn listen(sock: &Socket, backlog: u32) -> Result<(), i32> {
    let port = sock.port.load(Ordering::Relaxed) as u16;
    if sock.kind != Kind::Stream || sock.id().is_some() || port == 0 {
        return Err(err::EINVAL);
    }
    let id = with_stack(|s| s.tcp_listen(port, backlog as usize))?.map_err(errno)?;
    *sock.id.lock() = Some(id);
    Ok(())
}

/// つながってきた接続を受け取る（来るまで待つ）
pub fn accept(sock: &Socket) -> Result<(Arc<Socket>, Ipv4, u16), i32> {
    let id = sock.id().filter(|_| sock.kind == Kind::Stream).ok_or(err::EINVAL)?;
    loop {
        match with_stack(|s| s.tcp_accept(id))? {
            Ok((child, ip, port)) => {
                let s = Socket { kind: Kind::Stream, id: IrqMutex::new(Some(child)), port: AtomicU32::new(0) };
                return Ok((Arc::new(s), ip, port));
            }
            Err(NetError::WouldBlock) => wait_for(id, None, |r| r.readable)?,
            Err(e) => return Err(errno(e)),
        }
    }
}

/// UDP・ICMP は 1 つのパケットとして送る。TCP は全部送り終わるまで待つ
pub fn send_to(sock: &Socket, data: &[u8], addr: Ipv4, port: u16) -> Result<usize, i32> {
    match sock.kind {
        Kind::Stream => send_all(sock, data),
        Kind::Dgram => {
            let id = udp_id(sock)?;
            with_stack(|s| s.udp_send(id, addr, port, data))?.map_err(errno)?;
            kick();
            Ok(data.len())
        }
        Kind::Icmp => {
            let id = sock.id().ok_or(err::EBADF)?;
            with_stack(|s| s.icmp_send(id, addr, data))?.map_err(errno)?;
            kick();
            Ok(data.len())
        }
    }
}

/// TCP で全部送る（送るバッファが空くのを待ちながら）
pub fn send_all(sock: &Socket, data: &[u8]) -> Result<usize, i32> {
    let id = sock.id().filter(|_| sock.kind == Kind::Stream).ok_or(err::ENOTCONN)?;
    let mut done = 0;
    while done < data.len() {
        match with_stack(|s| s.tcp_send(id, &data[done..]))? {
            Ok(n) => {
                done += n;
                kick();
            }
            Err(NetError::WouldBlock) => {
                if let Err(e) = wait_for(id, None, |r| r.writable) {
                    return if done > 0 { Ok(done) } else { Err(e) };
                }
            }
            Err(e) => return if done > 0 { Ok(done) } else { Err(errno(e)) },
        }
    }
    Ok(done)
}

/// 受け取る。(バイト数, 送り元, ポート)。TCP の 0 は相手が送り終えた
pub fn recv_from(sock: &Socket, buf: &mut [u8], timeout_ms: u32) -> Result<(usize, Ipv4, u16), i32> {
    let timeout = timeout_of(timeout_ms);
    let id = match sock.kind {
        Kind::Dgram => udp_id(sock)?,
        _ => sock.id().ok_or(err::ENOTCONN)?,
    };
    loop {
        let r = with_stack(|s| match sock.kind {
            Kind::Stream => s.tcp_recv(id, buf).map(|n| (n, Ipv4::UNSPECIFIED, 0)),
            Kind::Dgram => s.udp_recv(id).map(|(ip, port, data)| {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                (n, ip, port)
            }),
            Kind::Icmp => s.icmp_recv(id).map(|(ip, msg, _ttl)| {
                let n = msg.len().min(buf.len());
                buf[..n].copy_from_slice(&msg[..n]);
                (n, ip, 0)
            }),
        })?;
        match r {
            Ok(v) => {
                if sock.kind == Kind::Stream {
                    kick(); // 受信の窓が開いたことを知らせる
                }
                return Ok(v);
            }
            Err(NetError::WouldBlock) if timeout != Some(0) => wait_for(id, timeout, |r| r.readable)?,
            Err(NetError::WouldBlock) => return Err(err::EAGAIN),
            Err(e) => return Err(errno(e)),
        }
    }
}

/// (読める, 書ける)
pub fn readiness(sock: &Socket) -> (bool, bool) {
    let Some(id) = sock.id() else {
        // まだ何もしていない TCP・UDP は書ける（読むものは無い）
        return (false, true);
    };
    with_stack(|s| {
        let r = s.readiness(id);
        (r.readable, r.writable)
    })
    .unwrap_or((true, true))
}

/// poll: どれかの条件が成り立つまで待つ。`check` は今の状態を見て準備のできた数を返す
pub fn wait_any(timeout_ms: u32, mut check: impl FnMut() -> usize) -> Result<usize, i32> {
    let me = sched::current_id();
    let timeout = timeout_of(timeout_ms);
    let start = pit::ticks();
    let mut result = Ok(0);
    let mut cond = || {
        let n = check();
        if n > 0 {
            result = Ok(n);
            return true;
        }
        if crate::process::kill_pending(me) {
            result = Err(err::EINTR);
            return true;
        }
        timeout.is_some_and(|t| pit::ticks().wrapping_sub(start) >= t)
    };
    if timeout.is_some() {
        WaitQueue::wait_until_any(&[&SOCK_WAIT, pit::tick_queue()], &mut cond);
    } else {
        SOCK_WAIT.wait_until(&mut cond);
    }
    result
}
