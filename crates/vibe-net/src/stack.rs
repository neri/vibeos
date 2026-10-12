//! ネットワークスタック本体
//!
//! 1 つのインタフェース（Ethernet）とそのアドレス、ARP の表、ソケットを持つ。
//! 使う側は次を繰り返す:
//! 1. 受け取ったフレームを `receive` に渡す
//! 2. `poll` で時間の経過を伝える（再送・ARP の再問い合わせ・DHCP・自分宛てのパケットの処理）
//! 3. `transmit` で送るフレームを取り出してカードに渡す
//!
//! ソケットの操作はすべて待たない（できなければ `NetError::WouldBlock`）。待つのは使う側の仕事で、
//! `readiness` で「読める・書ける・つながった」を確かめる。
//! 自分宛て（127.x と自分のアドレス）のパケットはカードに出さず、次の `poll` で受け取る。

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::dhcp;
use crate::tcp::{Segment, Tcb, TcpState};
use crate::wire::{self, Arp, ETHERTYPE_ARP, ETHERTYPE_IPV4, PROTO_ICMP, PROTO_TCP, PROTO_UDP};
use crate::{Ipv4, Mac};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    /// アドレスがまだ決まっていない（DHCP の途中など）
    NotConfigured,
    /// ゲートウェイが無いので届けられない
    NoRoute,
    /// ARP に答えが無い
    HostUnreachable,
    AddrInUse,
    BadSocket,
    NotConnected,
    ConnRefused,
    ConnReset,
    TimedOut,
    WouldBlock,
    /// 1 つの UDP・ICMP のパケットに入らない（フラグメントは作らない）
    TooBig,
    InvalidArgument,
}

impl NetError {
    pub fn message(self) -> &'static str {
        match self {
            NetError::NotConfigured => "network not configured",
            NetError::NoRoute => "no route to host",
            NetError::HostUnreachable => "host unreachable",
            NetError::AddrInUse => "address in use",
            NetError::BadSocket => "bad socket",
            NetError::NotConnected => "not connected",
            NetError::ConnRefused => "connection refused",
            NetError::ConnReset => "connection reset",
            NetError::TimedOut => "timed out",
            NetError::WouldBlock => "would block",
            NetError::TooBig => "message too long",
            NetError::InvalidArgument => "invalid argument",
        }
    }
}

pub type Result<T> = core::result::Result<T, NetError>;

/// インタフェースのアドレスの設定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfConfig {
    pub addr: Ipv4,
    pub prefix: u8,
    pub gateway: Option<Ipv4>,
    pub dns: Option<Ipv4>,
}

impl IfConfig {
    pub fn netmask(&self) -> Ipv4 {
        Ipv4::netmask(self.prefix)
    }

    /// 同じネットワークにいる（ゲートウェイを通さずに届く）
    pub fn on_link(&self, ip: Ipv4) -> bool {
        let m = self.netmask().to_u32();
        ip.to_u32() & m == self.addr.to_u32() & m
    }

    /// ネットワークのブロードキャストアドレス
    pub fn broadcast(&self) -> Ipv4 {
        Ipv4::from_u32(self.addr.to_u32() | !self.netmask().to_u32())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub rx_frames: u32,
    pub rx_bytes: u32,
    pub tx_frames: u32,
    pub tx_bytes: u32,
    /// 自分宛てでない・壊れている・置き場所が無いなどで捨てたもの
    pub rx_dropped: u32,
}

pub type SocketId = usize;

/// ソケットの状態（待つ側が見る）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readiness {
    /// 読める（データ・接続の要求・終わり・エラーがある）
    pub readable: bool,
    /// 書ける（または書けないことが分かっている）
    pub writable: bool,
    /// 接続の手順が終わった（成功でも失敗でも）
    pub connected: bool,
}

/// ソケットの一覧（表示用）
#[derive(Debug, Clone, Copy)]
pub struct SocketInfo {
    pub id: SocketId,
    pub kind: &'static str,
    pub local_port: u16,
    pub remote: Option<(Ipv4, u16)>,
    pub state: &'static str,
    pub recv_queued: usize,
    pub send_queued: usize,
}

const ARP_TTL_MS: u64 = 10 * 60 * 1000;
const ARP_RETRY_MS: u64 = 1000;
const ARP_TRIES: u32 = 3;
/// ARP の答えを待っている間に溜めておくパケットの数（宛先ごと）
const ARP_QUEUE: usize = 16;
const UDP_QUEUE: usize = 32;
const ICMP_QUEUE: usize = 16;
const EPHEMERAL_FIRST: u16 = 49152;
/// 1 つのパケットに入る UDP のデータの最大（1500 - IP 20 - UDP 8）
pub const MAX_UDP_PAYLOAD: usize = 1472;

struct UdpSocket {
    port: u16,
    rx: VecDeque<(Ipv4, u16, Vec<u8>)>,
}

struct IcmpSocket {
    ident: u16,
    rx: VecDeque<(Ipv4, Vec<u8>, u8)>,
}

struct TcpSocket {
    tcb: Tcb,
    /// まだ accept されていない接続の待ち受けソケット
    parent: Option<SocketId>,
    /// アプリが持っている（close したか、まだ accept されていなければ false。終わったら片付ける）
    owned: bool,
}

struct Listener {
    port: u16,
    backlog: usize,
    /// つながって accept を待っている接続
    queue: VecDeque<SocketId>,
}

enum Socket {
    Udp(UdpSocket),
    Icmp(IcmpSocket),
    Tcp(TcpSocket),
    Listener(Listener),
}

struct ArpWait {
    ip: Ipv4,
    tries: u32,
    next_try: u64,
    /// 答えが来たら送る IP のパケット
    packets: Vec<Vec<u8>>,
}

pub struct Stack {
    mac: Mac,
    iface: Option<IfConfig>,
    dhcp: Option<dhcp::Client>,
    /// 設定ファイルで決めたもの（DHCP の値より優先）
    gateway_override: Option<Ipv4>,
    dns_override: Option<Ipv4>,
    /// 新しくアドレスが決まった（使う側に知らせる）
    config_event: bool,
    arp: BTreeMap<Ipv4, (Mac, u64)>,
    arp_wait: Vec<ArpWait>,
    tx: VecDeque<Vec<u8>>,
    loopback: VecDeque<Vec<u8>>,
    sockets: Vec<Option<Socket>>,
    now: u64,
    ident: u16,
    rng: u32,
    next_port: u16,
    /// TCP で知らせる受信の窓の上限
    recv_window: u32,
    pub stats: Stats,
}

impl Stack {
    /// アドレスの無いスタックを作る（`set_static` か `start_dhcp` で決める）。seed は乱数の種
    pub fn new(mac: Mac, seed: u32) -> Stack {
        let mut s = Stack {
            mac,
            iface: None,
            dhcp: None,
            gateway_override: None,
            dns_override: None,
            config_event: false,
            arp: BTreeMap::new(),
            arp_wait: Vec::new(),
            tx: VecDeque::new(),
            loopback: VecDeque::new(),
            sockets: Vec::new(),
            now: 0,
            ident: 1,
            rng: seed | 1,
            next_port: EPHEMERAL_FIRST,
            recv_window: crate::tcp::RECV_BUF as u32,
            stats: Stats::default(),
        };
        s.next_port = EPHEMERAL_FIRST + (s.random() % 1000) as u16;
        s
    }

    fn random(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// TCP で相手に知らせる受信の窓の上限（相手が ACK を待たずに送ってくる量）。カードの受信の
    /// バッファが小さいときに、一度に来たフレームがあふれないよう小さくする
    pub fn set_recv_window(&mut self, bytes: u32) {
        self.recv_window = bytes;
    }

    /// 固定のアドレスにする
    pub fn set_static(&mut self, cfg: IfConfig) {
        self.dhcp = None;
        self.iface = Some(cfg);
        self.config_event = true;
    }

    /// DHCP でアドレスをもらう（次の poll から問い合わせる）
    pub fn start_dhcp(&mut self) {
        let xid = self.random();
        self.dhcp = Some(dhcp::Client::new(self.mac, xid));
    }

    pub fn dhcp_enabled(&self) -> bool {
        self.dhcp.is_some()
    }

    /// DHCP の値より優先するゲートウェイと DNS サーバ
    pub fn set_overrides(&mut self, gateway: Option<Ipv4>, dns: Option<Ipv4>) {
        self.gateway_override = gateway;
        self.dns_override = dns;
    }

    /// 今の設定（優先する値を当てはめたもの）
    pub fn config(&self) -> Option<IfConfig> {
        self.iface.map(|mut c| {
            c.gateway = self.gateway_override.or(c.gateway);
            c.dns = self.dns_override.or(c.dns);
            c
        })
    }

    /// 前に呼んでから新しくアドレスが決まったら、その設定を返す
    pub fn take_config_event(&mut self) -> Option<IfConfig> {
        if core::mem::take(&mut self.config_event) { self.config() } else { None }
    }

    /// 時刻（ミリ秒）を進める。ソケットの操作の前に呼ぶ（送るものの時刻に使う）
    pub fn set_now(&mut self, now: u64) {
        self.now = self.now.max(now);
    }

    // --- 送る -------------------------------------------------------------------

    /// カードに渡すフレームを 1 つ取り出す
    pub fn transmit(&mut self) -> Option<Vec<u8>> {
        let f = self.tx.pop_front()?;
        self.stats.tx_frames += 1;
        self.stats.tx_bytes = self.stats.tx_bytes.wrapping_add(f.len() as u32);
        Some(f)
    }

    fn push_frame(&mut self, dst: Mac, ethertype: u16, payload: &[u8]) {
        let f = wire::build_eth(dst, self.mac, ethertype, payload);
        self.tx.push_back(f);
    }

    fn send_arp(&mut self, op: u16, target_mac: Mac, target_ip: Ipv4) {
        let Some(cfg) = self.iface else { return };
        let a = Arp { op, sender_mac: self.mac, sender_ip: cfg.addr, target_mac, target_ip };
        let dst = if op == wire::ARP_REQUEST { Mac::BROADCAST } else { target_mac };
        self.push_frame(dst, ETHERTYPE_ARP, &a.to_bytes());
    }

    /// 宛先に使う送り元のアドレス
    fn source_for(&self, dst: Ipv4) -> Result<Ipv4> {
        if dst.is_loopback() {
            return Ok(Ipv4::LOOPBACK);
        }
        self.iface.map(|c| c.addr).ok_or(NetError::NotConfigured)
    }

    fn is_local(&self, ip: Ipv4) -> bool {
        ip.is_loopback() || self.iface.is_some_and(|c| c.addr == ip)
    }

    /// IP のパケットを作って送る（ARP の答えが無ければ問い合わせて待つ）
    fn send_ip(&mut self, dst: Ipv4, proto: u8, payload: &[u8]) -> Result<()> {
        let src = self.source_for(dst)?;
        let ident = self.ident;
        self.ident = self.ident.wrapping_add(1);
        let pkt = wire::build_ipv4(src, dst, proto, wire::DEFAULT_TTL, ident, payload);
        if self.is_local(dst) {
            self.loopback.push_back(pkt);
            return Ok(());
        }
        let cfg = self.config().ok_or(NetError::NotConfigured)?;
        if dst == Ipv4::BROADCAST || dst == cfg.broadcast() {
            self.push_frame(Mac::BROADCAST, ETHERTYPE_IPV4, &pkt);
            return Ok(());
        }
        let next_hop = if cfg.on_link(dst) { dst } else { cfg.gateway.ok_or(NoRoute)? };
        if let Some(mac) = self.arp_lookup(next_hop) {
            self.push_frame(mac, ETHERTYPE_IPV4, &pkt);
            return Ok(());
        }
        // ARP の答えを待つ
        match self.arp_wait.iter_mut().find(|w| w.ip == next_hop) {
            Some(w) => {
                if w.packets.len() < ARP_QUEUE {
                    w.packets.push(pkt);
                }
            }
            None => {
                self.arp_wait.push(ArpWait { ip: next_hop, tries: 1, next_try: self.now + ARP_RETRY_MS, packets: alloc::vec![pkt] });
                self.send_arp(wire::ARP_REQUEST, Mac::ZERO, next_hop);
            }
        }
        Ok(())
    }

    // --- ARP --------------------------------------------------------------------

    /// ARP の表から引く（古いものは使わない）
    pub fn arp_lookup(&self, ip: Ipv4) -> Option<Mac> {
        self.arp.get(&ip).filter(|(_, exp)| *exp > self.now).map(|(m, _)| *m)
    }

    /// 表に無ければ ARP で問い合わせる（答えは `arp_lookup` で見る）
    pub fn arp_query(&mut self, ip: Ipv4) -> Result<()> {
        let cfg = self.config().ok_or(NetError::NotConfigured)?;
        if !cfg.on_link(ip) {
            return Err(NetError::NoRoute);
        }
        if self.arp_lookup(ip).is_none() && !self.arp_wait.iter().any(|w| w.ip == ip) {
            self.arp_wait.push(ArpWait { ip, tries: 1, next_try: self.now + ARP_RETRY_MS, packets: Vec::new() });
            self.send_arp(wire::ARP_REQUEST, Mac::ZERO, ip);
        }
        Ok(())
    }

    /// ARP で問い合わせ中（まだ答えも諦めもしていない）
    pub fn arp_pending(&self, ip: Ipv4) -> bool {
        self.arp_wait.iter().any(|w| w.ip == ip)
    }

    /// ARP の表（表示用）
    pub fn arp_entries(&self) -> Vec<(Ipv4, Mac)> {
        self.arp.iter().filter(|(_, (_, e))| *e > self.now).map(|(ip, (m, _))| (*ip, *m)).collect()
    }

    fn arp_learn(&mut self, ip: Ipv4, mac: Mac) {
        self.arp.insert(ip, (mac, self.now + ARP_TTL_MS));
        if let Some(i) = self.arp_wait.iter().position(|w| w.ip == ip) {
            let w = self.arp_wait.swap_remove(i);
            for pkt in w.packets {
                self.push_frame(mac, ETHERTYPE_IPV4, &pkt);
            }
        }
    }

    fn on_arp(&mut self, p: &[u8]) {
        let (Some(a), Some(cfg)) = (Arp::parse(p), self.iface) else {
            self.stats.rx_dropped += 1;
            return;
        };
        if a.sender_ip.is_unspecified() || !cfg.on_link(a.sender_ip) || a.sender_mac.is_multicast() {
            return;
        }
        let for_me = a.target_ip == cfg.addr;
        // 表にあるものは更新し、自分への問い合わせなら相手を覚える（RFC 826）
        if for_me || self.arp.contains_key(&a.sender_ip) || self.arp_wait.iter().any(|w| w.ip == a.sender_ip) {
            self.arp_learn(a.sender_ip, a.sender_mac);
        }
        if for_me && a.op == wire::ARP_REQUEST {
            self.send_arp(wire::ARP_REPLY, a.sender_mac, a.sender_ip);
        }
    }

    // --- 受け取る -----------------------------------------------------------------

    /// カードから受け取ったフレームを処理する
    pub fn receive(&mut self, frame: &[u8], now: u64) {
        self.set_now(now);
        self.stats.rx_frames += 1;
        self.stats.rx_bytes = self.stats.rx_bytes.wrapping_add(frame.len() as u32);
        let Some(eth) = wire::parse_eth(frame) else {
            self.stats.rx_dropped += 1;
            return;
        };
        if eth.dst != self.mac && !eth.dst.is_broadcast() {
            self.stats.rx_dropped += 1;
            return;
        }
        match eth.ethertype {
            ETHERTYPE_ARP => self.on_arp(eth.payload),
            ETHERTYPE_IPV4 => self.on_ip(eth.payload, true),
            _ => self.stats.rx_dropped += 1,
        }
    }

    /// IP のパケットを処理する。from_wire はカードから来たもの（自分宛ての折り返しでない）
    fn on_ip(&mut self, bytes: &[u8], from_wire: bool) {
        let Ok(p) = wire::parse_ipv4(bytes) else {
            self.stats.rx_dropped += 1;
            return;
        };
        // 127.x は外から来るはずがない
        if from_wire && (p.dst.is_loopback() || p.src.is_loopback()) {
            self.stats.rx_dropped += 1;
            return;
        }
        let for_me = match self.iface {
            Some(c) => p.dst == c.addr || p.dst == Ipv4::BROADCAST || p.dst == c.broadcast() || p.dst.is_loopback(),
            None => p.dst == Ipv4::BROADCAST || p.dst.is_loopback(),
        };
        // DHCP の返事は、アドレスが決まる前に自分の（まだ使っていない）アドレス宛てで来ることがある
        let dhcp_reply = p.proto == PROTO_UDP && self.dhcp.is_some();
        if !for_me && !dhcp_reply {
            self.stats.rx_dropped += 1;
            return;
        }
        let broadcast = p.dst == Ipv4::BROADCAST || self.iface.is_some_and(|c| p.dst == c.broadcast());
        match p.proto {
            PROTO_ICMP if for_me => self.on_icmp(p.src, broadcast, p.ttl, p.payload),
            PROTO_UDP => self.on_udp(p.src, p.dst, for_me, broadcast, p.payload, bytes),
            PROTO_TCP if for_me && !broadcast => self.on_tcp(p.src, p.dst, p.payload),
            _ => self.stats.rx_dropped += 1,
        }
    }

    fn on_icmp(&mut self, src: Ipv4, broadcast: bool, ttl: u8, payload: &[u8]) {
        let Some(m) = wire::parse_icmp(payload) else {
            self.stats.rx_dropped += 1;
            return;
        };
        match m.kind {
            wire::ICMP_ECHO_REQUEST if !broadcast => {
                let reply = wire::build_icmp(wire::ICMP_ECHO_REPLY, 0, m.rest, m.data);
                let _ = self.send_ip(src, PROTO_ICMP, &reply);
            }
            wire::ICMP_ECHO_REPLY => self.deliver_icmp(m.ident(), src, payload, ttl),
            wire::ICMP_DEST_UNREACHABLE => {
                // 中身は届かなかったパケットの IP ヘッダと先頭 8 バイト。エコー要求なら送ったソケットへ
                if let Some(ident) = embedded_echo_ident(m.data) {
                    self.deliver_icmp(ident, src, payload, ttl);
                }
            }
            _ => {}
        }
    }

    fn deliver_icmp(&mut self, ident: u16, src: Ipv4, msg: &[u8], ttl: u8) {
        for s in self.sockets.iter_mut().flatten() {
            if let Socket::Icmp(i) = s {
                if i.ident == ident {
                    if i.rx.len() < ICMP_QUEUE {
                        i.rx.push_back((src, msg.to_vec(), ttl));
                    }
                    return;
                }
            }
        }
    }

    fn on_udp(&mut self, src: Ipv4, dst: Ipv4, for_me: bool, broadcast: bool, payload: &[u8], ip_packet: &[u8]) {
        let Some(u) = wire::parse_udp(src, dst, payload) else {
            self.stats.rx_dropped += 1;
            return;
        };
        if u.dst_port == dhcp::CLIENT_PORT && u.src_port == dhcp::SERVER_PORT {
            if let Some(d) = self.dhcp.as_mut() {
                if let Some(lease) = d.receive(u.payload, self.now) {
                    let prefix = lease.netmask.prefix_len().unwrap_or(24);
                    self.iface = Some(IfConfig { addr: lease.addr, prefix, gateway: lease.router, dns: lease.dns });
                    self.config_event = true;
                }
            }
            return;
        }
        if !for_me {
            return;
        }
        for s in self.sockets.iter_mut().flatten() {
            if let Socket::Udp(sock) = s {
                if sock.port == u.dst_port {
                    if sock.rx.len() < UDP_QUEUE {
                        sock.rx.push_back((src, u.src_port, u.payload.to_vec()));
                    } else {
                        self.stats.rx_dropped += 1;
                    }
                    return;
                }
            }
        }
        // 誰も待っていない: ポートが閉じていると知らせる
        if !broadcast {
            let n = ip_packet.len().min(wire::IP_HEADER + 8);
            let msg = wire::build_icmp(wire::ICMP_DEST_UNREACHABLE, wire::ICMP_PORT_UNREACHABLE, [0; 4], &ip_packet[..n]);
            let _ = self.send_ip(src, PROTO_ICMP, &msg);
        }
    }

    fn on_tcp(&mut self, src: Ipv4, dst: Ipv4, payload: &[u8]) {
        let Some(seg) = wire::parse_tcp(src, dst, payload) else {
            self.stats.rx_dropped += 1;
            return;
        };
        let h = seg.header;
        let now = self.now;
        if let Some(id) = self.find_tcp(h.dst_port, src, h.src_port) {
            let mut out = Vec::new();
            let became_established = {
                let Some(Socket::Tcp(t)) = self.sockets[id].as_mut() else { unreachable!() };
                let before = t.tcb.state;
                t.tcb.on_segment(&h, seg.payload, now, &mut out);
                t.tcb.output(now, &mut out);
                before == TcpState::SynReceived && t.tcb.state != TcpState::SynReceived && t.tcb.state != TcpState::Closed
            };
            self.emit(id, out);
            if became_established {
                self.to_accept_queue(id);
            }
            self.gc();
            return;
        }
        let rst = h.flags & wire::TCP_RST != 0;
        if let Some(lid) = self.find_listener(h.dst_port) {
            if rst {
                return;
            }
            if h.flags & wire::TCP_ACK != 0 {
                self.send_reset(src, &h, seg.payload.len());
                return;
            }
            if h.flags & wire::TCP_SYN != 0 {
                let Some(Socket::Listener(l)) = &self.sockets[lid] else { return };
                let backlog = l.backlog;
                let children = self.sockets.iter().flatten().filter(|s| matches!(s, Socket::Tcp(t) if t.parent == Some(lid))).count();
                if children >= backlog {
                    return; // 一杯: 相手が SYN を送り直すのを待つ
                }
                let iss = self.new_iss();
                let mut tcb = Tcb::from_syn(h.dst_port, (src, h.src_port), &h, iss);
                tcb.set_window_limit(self.recv_window);
                let id = self.insert(Socket::Tcp(TcpSocket { tcb, parent: Some(lid), owned: false }));
                let mut out = Vec::new();
                if let Some(Socket::Tcp(t)) = self.sockets[id].as_mut() {
                    t.tcb.output(now, &mut out);
                }
                self.emit(id, out);
            }
            return;
        }
        if !rst {
            self.send_reset(src, &h, seg.payload.len());
        }
    }

    /// 相手の無いセグメントへの RST
    fn send_reset(&mut self, src: Ipv4, h: &wire::TcpHeader, len: usize) {
        let mut r = wire::TcpHeader { src_port: h.dst_port, dst_port: h.src_port, ..Default::default() };
        if h.flags & wire::TCP_ACK != 0 {
            r.seq = h.ack;
            r.flags = wire::TCP_RST;
        } else {
            let seg_len = len as u32 + (h.flags & wire::TCP_SYN != 0) as u32 + (h.flags & wire::TCP_FIN != 0) as u32;
            r.ack = h.seq.wrapping_add(seg_len);
            r.flags = wire::TCP_RST | wire::TCP_ACK;
        }
        if let Ok(local) = self.source_for(src) {
            let bytes = wire::build_tcp(local, src, &r, &[]);
            let _ = self.send_ip(src, PROTO_TCP, &bytes);
        }
    }

    fn new_iss(&mut self) -> u32 {
        // 時刻（4µs ごとに 1 進む時計の近似）と乱数を混ぜる
        self.random() ^ (self.now as u32).wrapping_mul(250)
    }

    fn to_accept_queue(&mut self, id: SocketId) {
        let Some(Socket::Tcp(t)) = &self.sockets[id] else { return };
        let Some(lid) = t.parent else { return };
        if let Some(Socket::Listener(l)) = self.sockets[lid].as_mut() {
            l.queue.push_back(id);
        }
    }

    /// TCP のセグメントを IP で包んで送る
    fn emit(&mut self, id: SocketId, out: Vec<Segment>) {
        let Some(Socket::Tcp(t)) = &self.sockets[id] else { return };
        let remote = t.tcb.remote.0;
        let Ok(local) = self.source_for(remote) else { return };
        for seg in out {
            let bytes = wire::build_tcp(local, remote, &seg.header, &seg.payload);
            let _ = self.send_ip(remote, PROTO_TCP, &bytes);
        }
    }

    // --- 時間の経過 ---------------------------------------------------------------

    /// 時間の経過を伝える: 自分宛てのパケット・DHCP・ARP の再問い合わせ・TCP の再送
    pub fn poll(&mut self, now: u64) {
        self.set_now(now);
        let now = self.now;
        // 自分宛て（処理中に増えた分は次の回に）
        for _ in 0..self.loopback.len() {
            if let Some(p) = self.loopback.pop_front() {
                self.on_ip(&p, false);
            }
        }
        if let Some(msg) = self.dhcp.as_mut().and_then(|d| d.poll(now)) {
            let src = self.iface.map_or(Ipv4::UNSPECIFIED, |c| c.addr);
            let udp = wire::build_udp(src, Ipv4::BROADCAST, dhcp::CLIENT_PORT, dhcp::SERVER_PORT, &msg);
            let ident = self.ident;
            self.ident = self.ident.wrapping_add(1);
            let pkt = wire::build_ipv4(src, Ipv4::BROADCAST, PROTO_UDP, wire::DEFAULT_TTL, ident, &udp);
            self.push_frame(Mac::BROADCAST, ETHERTYPE_IPV4, &pkt);
        }
        let mut i = 0;
        while i < self.arp_wait.len() {
            let w = &mut self.arp_wait[i];
            if now < w.next_try {
                i += 1;
                continue;
            }
            if w.tries >= ARP_TRIES {
                // 答えが無い: 溜めていたパケットは捨てる
                self.arp_wait.swap_remove(i);
                continue;
            }
            w.tries += 1;
            w.next_try = now + ARP_RETRY_MS;
            let ip = w.ip;
            self.send_arp(wire::ARP_REQUEST, Mac::ZERO, ip);
            i += 1;
        }
        for id in 0..self.sockets.len() {
            let mut out = Vec::new();
            if let Some(Socket::Tcp(t)) = self.sockets[id].as_mut() {
                t.tcb.poll(now, &mut out);
            }
            if !out.is_empty() {
                self.emit(id, out);
            }
        }
        self.gc();
    }

    /// 次に poll を呼ぶべき時刻（今すぐなら現在の時刻、することが無ければ None）
    pub fn next_deadline(&self) -> Option<u64> {
        let mut t: Option<u64> = None;
        let mut take = |v: u64| t = Some(t.map_or(v, |x| x.min(v)));
        if !self.loopback.is_empty() {
            take(self.now);
        }
        if let Some(d) = &self.dhcp {
            take(d.next_deadline());
        }
        for w in &self.arp_wait {
            take(w.next_try);
        }
        for s in self.sockets.iter().flatten() {
            if let Socket::Tcp(t) = s {
                if let Some(d) = t.tcb.next_deadline() {
                    take(d);
                }
            }
        }
        t
    }

    /// 送るフレームがある
    pub fn has_tx(&self) -> bool {
        !self.tx.is_empty()
    }

    // --- ソケット -----------------------------------------------------------------

    fn insert(&mut self, s: Socket) -> SocketId {
        if let Some(i) = self.sockets.iter().position(Option::is_none) {
            self.sockets[i] = Some(s);
            return i;
        }
        self.sockets.push(Some(s));
        self.sockets.len() - 1
    }

    fn port_in_use(&self, port: u16, tcp: bool) -> bool {
        self.sockets.iter().flatten().any(|s| match s {
            Socket::Udp(u) => !tcp && u.port == port,
            Socket::Tcp(t) => tcp && t.tcb.local_port == port,
            Socket::Listener(l) => tcp && l.port == port,
            Socket::Icmp(_) => false,
        })
    }

    fn ephemeral_port(&mut self, tcp: bool) -> u16 {
        loop {
            let p = self.next_port;
            self.next_port = if p == u16::MAX { EPHEMERAL_FIRST } else { p + 1 };
            if !self.port_in_use(p, tcp) {
                return p;
            }
        }
    }

    fn find_tcp(&self, local_port: u16, ip: Ipv4, port: u16) -> Option<SocketId> {
        self.sockets.iter().position(|s| {
            matches!(s, Some(Socket::Tcp(t)) if t.tcb.local_port == local_port && t.tcb.remote == (ip, port) && t.tcb.state != TcpState::Closed)
        })
    }

    fn find_listener(&self, port: u16) -> Option<SocketId> {
        self.sockets.iter().position(|s| matches!(s, Some(Socket::Listener(l)) if l.port == port))
    }

    /// UDP のソケットを作る（port 0 なら空いている番号）
    pub fn udp_open(&mut self, port: u16) -> Result<SocketId> {
        let port = if port == 0 { self.ephemeral_port(false) } else { port };
        if self.port_in_use(port, false) {
            return Err(NetError::AddrInUse);
        }
        Ok(self.insert(Socket::Udp(UdpSocket { port, rx: VecDeque::new() })))
    }

    pub fn udp_send(&mut self, id: SocketId, dst: Ipv4, port: u16, data: &[u8]) -> Result<()> {
        if data.len() > MAX_UDP_PAYLOAD {
            return Err(NetError::TooBig);
        }
        let Some(Socket::Udp(u)) = self.sockets.get(id).and_then(Option::as_ref) else { return Err(NetError::BadSocket) };
        let sport = u.port;
        let src = self.source_for(dst)?;
        let bytes = wire::build_udp(src, dst, sport, port, data);
        self.send_ip(dst, PROTO_UDP, &bytes)
    }

    /// 届いた UDP のデータ（送り元のアドレス, ポート, 中身）
    pub fn udp_recv(&mut self, id: SocketId) -> Result<(Ipv4, u16, Vec<u8>)> {
        match self.sockets.get_mut(id).and_then(Option::as_mut) {
            Some(Socket::Udp(u)) => u.rx.pop_front().ok_or(NetError::WouldBlock),
            _ => Err(NetError::BadSocket),
        }
    }

    /// ICMP のエコー（ping）用のソケットを作る
    pub fn icmp_open(&mut self) -> Result<SocketId> {
        let ident = loop {
            let v = (self.random() >> 8) as u16;
            if !self.sockets.iter().flatten().any(|s| matches!(s, Socket::Icmp(i) if i.ident == v)) {
                break v;
            }
        };
        Ok(self.insert(Socket::Icmp(IcmpSocket { ident, rx: VecDeque::new() })))
    }

    /// ICMP のメッセージを送る。msg はエコー要求の全体（種類・コード・識別子・番号・データ）で、
    /// 識別子とチェックサムはここで書き換える
    pub fn icmp_send(&mut self, id: SocketId, dst: Ipv4, msg: &[u8]) -> Result<()> {
        if msg.len() < wire::ICMP_HEADER || msg[0] != wire::ICMP_ECHO_REQUEST {
            return Err(NetError::InvalidArgument);
        }
        if msg.len() > MAX_UDP_PAYLOAD + wire::UDP_HEADER {
            return Err(NetError::TooBig);
        }
        let Some(Socket::Icmp(i)) = self.sockets.get(id).and_then(Option::as_ref) else { return Err(NetError::BadSocket) };
        let mut rest = [0u8; 4];
        rest.copy_from_slice(&msg[4..8]);
        rest[0..2].copy_from_slice(&i.ident.to_be_bytes());
        let m = wire::build_icmp(msg[0], msg[1], rest, &msg[8..]);
        self.send_ip(dst, PROTO_ICMP, &m)
    }

    /// 届いた ICMP のメッセージ（送り元, メッセージ全体, TTL）
    pub fn icmp_recv(&mut self, id: SocketId) -> Result<(Ipv4, Vec<u8>, u8)> {
        match self.sockets.get_mut(id).and_then(Option::as_mut) {
            Some(Socket::Icmp(i)) => i.rx.pop_front().ok_or(NetError::WouldBlock),
            _ => Err(NetError::BadSocket),
        }
    }

    /// TCP でつなぎに行く（待たない。つながったかは readiness で見る）
    pub fn tcp_connect(&mut self, dst: Ipv4, port: u16) -> Result<SocketId> {
        if port == 0 || dst.is_unspecified() || dst == Ipv4::BROADCAST {
            return Err(NetError::InvalidArgument);
        }
        self.source_for(dst)?;
        let local = self.ephemeral_port(true);
        let iss = self.new_iss();
        let mut tcb = Tcb::connect(local, (dst, port), iss);
        tcb.set_window_limit(self.recv_window);
        let id = self.insert(Socket::Tcp(TcpSocket { tcb, parent: None, owned: true }));
        self.output(id);
        Ok(id)
    }

    /// ポートで待ち受ける
    pub fn tcp_listen(&mut self, port: u16, backlog: usize) -> Result<SocketId> {
        if port == 0 {
            return Err(NetError::InvalidArgument);
        }
        if self.port_in_use(port, true) {
            return Err(NetError::AddrInUse);
        }
        Ok(self.insert(Socket::Listener(Listener { port, backlog: backlog.clamp(1, 16), queue: VecDeque::new() })))
    }

    /// つながった接続を 1 つ受け取る（新しいソケット, 相手のアドレス, ポート）
    pub fn tcp_accept(&mut self, id: SocketId) -> Result<(SocketId, Ipv4, u16)> {
        let Some(Socket::Listener(l)) = self.sockets.get_mut(id).and_then(Option::as_mut) else {
            return Err(NetError::BadSocket);
        };
        let child = l.queue.pop_front().ok_or(NetError::WouldBlock)?;
        let Some(Socket::Tcp(t)) = self.sockets[child].as_mut() else { return Err(NetError::BadSocket) };
        t.parent = None;
        t.owned = true;
        Ok((child, t.tcb.remote.0, t.tcb.remote.1))
    }

    fn tcb_mut(&mut self, id: SocketId) -> Result<&mut Tcb> {
        match self.sockets.get_mut(id).and_then(Option::as_mut) {
            Some(Socket::Tcp(t)) if t.owned => Ok(&mut t.tcb),
            _ => Err(NetError::BadSocket),
        }
    }

    fn output(&mut self, id: SocketId) {
        let now = self.now;
        let mut out = Vec::new();
        if let Some(Socket::Tcp(t)) = self.sockets[id].as_mut() {
            t.tcb.output(now, &mut out);
        }
        self.emit(id, out);
    }

    /// 送るデータを渡す。受け付けたバイト数を返す（一杯なら WouldBlock）
    pub fn tcp_send(&mut self, id: SocketId, data: &[u8]) -> Result<usize> {
        let n = self.tcb_mut(id)?.send(data)?;
        self.output(id);
        Ok(n)
    }

    /// 受け取ったデータを読む（0 なら相手が送り終えた）
    pub fn tcp_recv(&mut self, id: SocketId, buf: &mut [u8]) -> Result<usize> {
        let n = self.tcb_mut(id)?.recv(buf)?;
        self.output(id); // 窓が開いたことを知らせる
        Ok(n)
    }

    pub fn tcp_state(&self, id: SocketId) -> Option<TcpState> {
        match self.sockets.get(id)?.as_ref()? {
            Socket::Tcp(t) => Some(t.tcb.state),
            _ => None,
        }
    }

    /// 接続の失敗の理由（つながっていれば None）
    pub fn tcp_error(&self, id: SocketId) -> Option<NetError> {
        match self.sockets.get(id)?.as_ref()? {
            Socket::Tcp(t) => t.tcb.error.or((t.tcb.state == TcpState::Closed).then_some(NetError::NotConnected)),
            _ => None,
        }
    }

    /// 送ったデータがすべて相手に届いた
    pub fn tcp_all_acked(&self, id: SocketId) -> bool {
        matches!(self.sockets.get(id).and_then(Option::as_ref), Some(Socket::Tcp(t)) if t.tcb.all_acked())
    }

    pub fn readiness(&self, id: SocketId) -> Readiness {
        let all = Readiness { readable: true, writable: true, connected: true };
        match self.sockets.get(id).and_then(Option::as_ref) {
            None => all,
            Some(Socket::Udp(u)) => Readiness { readable: !u.rx.is_empty(), ..all },
            Some(Socket::Icmp(i)) => Readiness { readable: !i.rx.is_empty(), ..all },
            Some(Socket::Listener(l)) => Readiness { readable: !l.queue.is_empty(), ..all },
            Some(Socket::Tcp(t)) => Readiness {
                readable: t.tcb.readable(),
                writable: t.tcb.writable(),
                connected: t.tcb.connect_done(),
            },
        }
    }

    /// ソケットを閉じる。TCP は送り終えてから（FIN）片付ける
    pub fn close(&mut self, id: SocketId) {
        let now = self.now;
        let mut out = Vec::new();
        match self.sockets.get_mut(id).and_then(Option::as_mut) {
            None => return,
            Some(Socket::Udp(_)) | Some(Socket::Icmp(_)) => self.sockets[id] = None,
            Some(Socket::Tcp(t)) => {
                t.owned = false;
                t.tcb.close(&mut out);
                t.tcb.output(now, &mut out);
            }
            Some(Socket::Listener(_)) => {
                // まだ accept されていない接続は切る
                for cid in 0..self.sockets.len() {
                    let mut child_out = Vec::new();
                    if let Some(Socket::Tcp(t)) = self.sockets[cid].as_mut() {
                        if t.parent == Some(id) {
                            t.parent = None;
                            t.tcb.abort(&mut child_out);
                        }
                    }
                    self.emit(cid, child_out);
                }
                self.sockets[id] = None;
            }
        }
        self.emit(id, out);
        self.gc();
    }

    /// 終わった TCP の接続（アプリが手放したもの）を片付ける
    fn gc(&mut self) {
        for id in 0..self.sockets.len() {
            let parent = match &self.sockets[id] {
                Some(Socket::Tcp(t)) if !t.owned && t.tcb.state == TcpState::Closed => t.parent,
                _ => continue,
            };
            self.sockets[id] = None;
            if let Some(Some(Socket::Listener(l))) = parent.map(|p| self.sockets[p].as_mut()) {
                l.queue.retain(|&c| c != id);
            }
        }
    }

    /// ソケットの一覧（表示用）
    pub fn socket_list(&self) -> Vec<SocketInfo> {
        let mut v = Vec::new();
        for (id, s) in self.sockets.iter().enumerate() {
            let Some(s) = s else { continue };
            v.push(match s {
                Socket::Udp(u) => SocketInfo {
                    id,
                    kind: "udp",
                    local_port: u.port,
                    remote: None,
                    state: "",
                    recv_queued: u.rx.len(),
                    send_queued: 0,
                },
                Socket::Icmp(i) => SocketInfo {
                    id,
                    kind: "icmp",
                    local_port: i.ident,
                    remote: None,
                    state: "",
                    recv_queued: i.rx.len(),
                    send_queued: 0,
                },
                Socket::Listener(l) => SocketInfo {
                    id,
                    kind: "tcp",
                    local_port: l.port,
                    remote: None,
                    state: "LISTEN",
                    recv_queued: l.queue.len(),
                    send_queued: 0,
                },
                Socket::Tcp(t) => SocketInfo {
                    id,
                    kind: "tcp",
                    local_port: t.tcb.local_port,
                    remote: Some(t.tcb.remote),
                    state: t.tcb.state.name(),
                    recv_queued: t.tcb.recv_queued(),
                    send_queued: t.tcb.send_queued(),
                },
            });
        }
        v
    }
}

use NetError::NoRoute;

/// 「届かなかった」の ICMP に入っている元のパケットがエコー要求なら、その識別子
fn embedded_echo_ident(data: &[u8]) -> Option<u16> {
    let ihl = (*data.first()? & 0x0F) as usize * 4;
    if data.get(9) != Some(&PROTO_ICMP) {
        return None;
    }
    let icmp = data.get(ihl..ihl + 8)?;
    (icmp[0] == wire::ICMP_ECHO_REQUEST).then(|| u16::from_be_bytes([icmp[4], icmp[5]]))
}
