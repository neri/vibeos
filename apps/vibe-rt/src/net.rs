//! ネットワーク（ソケットのシステムコールの薄いラッパと、名前解決・TCP の入れ物）
//!
//! エラーは負のエラー番号（`vibe_abi::err`）。`err_name` で文字にできる。

use alloc::vec::Vec;

pub use vibe_abi::{NetInfo, PollFd, SockAddr};
use vibe_abi::{SOCK_DGRAM, SOCK_STREAM, err, nr};
pub use vibe_net::{Ipv4, dns, http, sntp};

use crate::sys::{self, syscall};

pub fn socket(kind: u32) -> i32 {
    syscall(nr::SOCKET, kind, 0, 0, 0, 0)
}

pub fn bind(fd: i32, port: u16) -> i32 {
    syscall(nr::BIND, fd as u32, port as u32, 0, 0, 0)
}

pub fn connect(fd: i32, addr: Ipv4, port: u16) -> i32 {
    let sa = SockAddr::new(addr.0, port);
    syscall(nr::CONNECT, fd as u32, &sa as *const SockAddr as u32, 0, 0, 0)
}

pub fn listen(fd: i32, backlog: u32) -> i32 {
    syscall(nr::LISTEN, fd as u32, backlog, 0, 0, 0)
}

/// つながってきた接続の (fd, 相手のアドレス, ポート)
pub fn accept(fd: i32) -> Result<(i32, Ipv4, u16), i32> {
    let mut sa = SockAddr::default();
    let r = syscall(nr::ACCEPT, fd as u32, &mut sa as *mut SockAddr as u32, 0, 0, 0);
    if r < 0 { Err(-r) } else { Ok((r, Ipv4(sa.addr), sa.port)) }
}

pub fn sendto(fd: i32, buf: &[u8], addr: Ipv4, port: u16) -> i32 {
    let sa = SockAddr::new(addr.0, port);
    syscall(nr::SENDTO, fd as u32, buf.as_ptr() as u32, buf.len() as u32, &sa as *const SockAddr as u32, 0)
}

/// 受け取る: (バイト数, 送り元, ポート)。timeout_ms = 0 なら待たない、u32::MAX ならいつまでも
pub fn recvfrom(fd: i32, buf: &mut [u8], timeout_ms: u32) -> Result<(usize, Ipv4, u16), i32> {
    let mut sa = SockAddr::default();
    let r = syscall(
        nr::RECVFROM,
        fd as u32,
        buf.as_mut_ptr() as u32,
        buf.len() as u32,
        &mut sa as *mut SockAddr as u32,
        timeout_ms,
    );
    if r < 0 { Err(-r) } else { Ok((r as usize, Ipv4(sa.addr), sa.port)) }
}

pub fn net_info() -> Result<NetInfo, i32> {
    let mut ni = NetInfo::default();
    let r = syscall(nr::NET_INFO, &mut ni as *mut NetInfo as u32, 0, 0, 0, 0);
    if r < 0 { Err(-r) } else { Ok(ni) }
}

/// どれかの準備ができるまで待つ。準備のできた数（0 なら時間切れ）
pub fn poll(fds: &mut [PollFd], timeout_ms: u32) -> i32 {
    syscall(nr::POLL, fds.as_mut_ptr() as u32, fds.len() as u32, timeout_ms, 0, 0)
}

/// Ctrl+C でプロセスを止めず、印を付けるだけにする（`ctrl_c_pressed` で見る）
pub fn catch_ctrl_c() {
    syscall(nr::CTRL_C, vibe_abi::CTRL_C_CATCH, 0, 0, 0, 0);
}

/// Ctrl+C が押されたか（印を消す）
pub fn ctrl_c_pressed() -> bool {
    syscall(nr::CTRL_C, vibe_abi::CTRL_C_TAKE, 0, 0, 0, 0) == 1
}

/// 時計を合わせる（1970 年からの秒, 現地時刻）
pub fn set_time(secs: u32) -> i32 {
    syscall(nr::SET_TIME, secs, 0, 0, 0, 0)
}

pub fn err_name(e: i32) -> &'static str {
    err::name(e)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// システムコールのエラー番号
    Sys(i32),
    Dns(dns::DnsError),
    NoServer,
    TimedOut,
}

impl ResolveError {
    pub fn message(self) -> &'static str {
        match self {
            ResolveError::Sys(e) => err::name(e),
            ResolveError::Dns(e) => e.message(),
            ResolveError::NoServer => "no DNS server (set dns= in BOOT.CFG)",
            ResolveError::TimedOut => "DNS server did not answer",
        }
    }
}

/// 名前（または "10.0.2.2" の形）をアドレスにする。DNS は net_info の DNS サーバに問い合わせる
pub fn resolve(name: &str) -> Result<Ipv4, ResolveError> {
    if let Some(ip) = Ipv4::parse(name) {
        return Ok(ip);
    }
    let info = net_info().map_err(ResolveError::Sys)?;
    let server = Ipv4(info.dns);
    if server.is_unspecified() {
        return Err(ResolveError::NoServer);
    }
    resolve_with(name, server, 2000, 3).map(|v| v[0])
}

/// DNS サーバ `server` に A レコードを問い合わせる（返事が無ければ tries 回まで送り直す）
pub fn resolve_with(name: &str, server: Ipv4, timeout_ms: u32, tries: u32) -> Result<Vec<Ipv4>, ResolveError> {
    let fd = Socket::new(SOCK_DGRAM).map_err(ResolveError::Sys)?;
    let id = (sys::ticks() as u16) ^ (sys::getpid() as u16).rotate_left(8);
    let query = dns::build_query(id, name).map_err(ResolveError::Dns)?;
    let mut buf = [0u8; 1500];
    for _ in 0..tries {
        let r = sendto(fd.0, &query, server, dns::PORT);
        if r < 0 {
            return Err(ResolveError::Sys(-r));
        }
        let start = sys::ticks();
        loop {
            let elapsed = sys::ticks().wrapping_sub(start) * (1000 / vibe_abi::TICK_HZ);
            if elapsed >= timeout_ms {
                break;
            }
            match recvfrom(fd.0, &mut buf, timeout_ms - elapsed) {
                Ok((n, from, port)) if from == server && port == dns::PORT => {
                    match dns::parse_response(&buf[..n], id) {
                        Err(dns::DnsError::WrongId) => continue,
                        r => return r.map_err(ResolveError::Dns),
                    }
                }
                Ok(_) => continue,
                Err(err::ETIMEDOUT) => break,
                Err(e) => return Err(ResolveError::Sys(e)),
            }
        }
    }
    Err(ResolveError::TimedOut)
}

/// 閉じ忘れないためのソケットの入れ物（捨てると close）
pub struct Socket(pub i32);

impl Socket {
    pub fn new(kind: u32) -> Result<Socket, i32> {
        let fd = socket(kind);
        if fd < 0 { Err(-fd) } else { Ok(Socket(fd)) }
    }

    /// TCP でつなぐ
    pub fn connect(addr: Ipv4, port: u16) -> Result<Socket, i32> {
        let s = Socket::new(SOCK_STREAM)?;
        let r = connect(s.0, addr, port);
        if r < 0 { Err(-r) } else { Ok(s) }
    }

    /// TCP で待ち受ける
    pub fn listen(port: u16, backlog: u32) -> Result<Socket, i32> {
        let s = Socket::new(SOCK_STREAM)?;
        let r = bind(s.0, port);
        if r < 0 {
            return Err(-r);
        }
        let r = listen(s.0, backlog);
        if r < 0 { Err(-r) } else { Ok(s) }
    }

    /// 全部書く（TCP）
    pub fn write_all(&self, data: &[u8]) -> Result<(), i32> {
        let r = sys::write(self.0, data);
        if r < 0 {
            Err(-r)
        } else if (r as usize) < data.len() {
            Err(err::ECONNRESET)
        } else {
            Ok(())
        }
    }

    /// 読む（0 なら相手が送り終えた）。timeout_ms は recvfrom と同じ
    pub fn read(&self, buf: &mut [u8], timeout_ms: u32) -> Result<usize, i32> {
        recvfrom(self.0, buf, timeout_ms).map(|r| r.0)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        sys::close(self.0);
    }
}
