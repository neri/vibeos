//! ハードウェアに依存しない TCP/IP（DESIGN.md §12）
//!
//! - `wire`: Ethernet / ARP / IPv4 / ICMP / UDP / TCP の組み立てと解析、チェックサム
//! - `stack`: ARP の表・IP の送り先の決定・ソケット（UDP・ICMP エコー・TCP）・DHCP をまとめたもの
//! - `tcp`: TCP の接続 1 本分の状態遷移・再送
//! - `dhcp` / `dns` / `sntp` / `http`: アプリ層のメッセージの組み立てと解析
//! - `config`: BOOT.CFG のネットワークの設定
//!
//! スタックはハードウェアと時計を持たない。使う側が「受け取ったフレームを渡す」「送るフレームを
//! 取り出す」「時刻（ミリ秒）を進める」を繰り返す。ホストの `cargo test` で、2 つのスタックを
//! 直結した通信やパケットを落とす回線を試せる。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod config;
pub mod dhcp;
pub mod dns;
pub mod http;
pub mod sntp;
pub mod stack;
pub mod tcp;
pub mod wire;

#[cfg(test)]
mod tests;

use core::fmt;

pub use stack::{IfConfig, NetError, Readiness, SocketId, Stack, Stats};
pub use tcp::TcpState;

/// IPv4 アドレス（ネットワークのバイト順のまま持つ）
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ipv4(pub [u8; 4]);

impl Ipv4 {
    pub const UNSPECIFIED: Ipv4 = Ipv4([0, 0, 0, 0]);
    pub const BROADCAST: Ipv4 = Ipv4([255, 255, 255, 255]);
    pub const LOOPBACK: Ipv4 = Ipv4([127, 0, 0, 1]);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Ipv4 {
        Ipv4([a, b, c, d])
    }

    pub fn to_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    pub fn from_u32(v: u32) -> Ipv4 {
        Ipv4(v.to_be_bytes())
    }

    pub fn is_unspecified(self) -> bool {
        self == Ipv4::UNSPECIFIED
    }

    pub fn is_loopback(self) -> bool {
        self.0[0] == 127
    }

    /// "10.0.2.15" の形を読む
    pub fn parse(s: &str) -> Option<Ipv4> {
        let mut out = [0u8; 4];
        let mut parts = s.split('.');
        for b in &mut out {
            let p = parts.next()?;
            if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            *b = p.parse().ok()?;
        }
        parts.next().is_none().then_some(Ipv4(out))
    }

    /// プレフィックス長からネットマスク
    pub fn netmask(prefix: u8) -> Ipv4 {
        let p = prefix.min(32) as u32;
        Ipv4::from_u32(if p == 0 { 0 } else { u32::MAX << (32 - p) })
    }

    /// ネットマスクからプレフィックス長（連続していなければ None）
    pub fn prefix_len(self) -> Option<u8> {
        let v = self.to_u32();
        let ones = v.leading_ones();
        (v.checked_shl(ones).unwrap_or(0) == 0).then_some(ones as u8)
    }
}

impl fmt::Display for Ipv4 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let [a, b, c, d] = self.0;
        // 幅の指定（{:15} など）が効くように、いったん固定長の文字列にしてから pad する
        let mut buf = [0u8; 15];
        let mut w = Cursor { buf: &mut buf, len: 0 };
        let _ = fmt::write(&mut w, format_args!("{a}.{b}.{c}.{d}"));
        let len = w.len;
        f.pad(core::str::from_utf8(&buf[..len]).unwrap_or("?"))
    }
}

impl fmt::Debug for Ipv4 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// MAC アドレス
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Mac(pub [u8; 6]);

impl Mac {
    pub const BROADCAST: Mac = Mac([0xFF; 6]);
    pub const ZERO: Mac = Mac([0; 6]);

    pub fn is_broadcast(self) -> bool {
        self == Mac::BROADCAST
    }

    pub fn is_multicast(self) -> bool {
        self.0[0] & 1 != 0
    }
}

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let m = self.0;
        write!(f, "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// ヒープを使わずに書式付きで書くための小さな書き込み先
struct Cursor<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl fmt::Write for Cursor<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
