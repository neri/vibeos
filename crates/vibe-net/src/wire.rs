//! パケットの組み立てと解析
//!
//! 解析は「正しい形をしているものだけ」を返す（長さが足りない・チェックサムが合わないものは None）。
//! 組み立ては `Vec<u8>` を返す。数値はすべてネットワークのバイト順（ビッグエンディアン）。

use alloc::vec::Vec;

use crate::{Ipv4, Mac};

// --- チェックサム -------------------------------------------------------------

/// 16bit ごとの 1 の補数の和（折り返す前）。奇数の長さは最後のバイトの後ろに 0 を足して数える
pub fn sum16(mut acc: u32, data: &[u8]) -> u32 {
    let mut chunks = data.chunks_exact(2);
    for c in &mut chunks {
        acc += u16::from_be_bytes([c[0], c[1]]) as u32;
    }
    if let [last] = chunks.remainder() {
        acc += (*last as u32) << 8;
    }
    acc
}

/// 和を 16bit に折り返して 1 の補数をとる
pub fn fold(mut acc: u32) -> u16 {
    while acc > 0xFFFF {
        acc = (acc & 0xFFFF) + (acc >> 16);
    }
    !(acc as u16)
}

/// インターネットチェックサム（RFC 1071）
pub fn checksum(data: &[u8]) -> u16 {
    fold(sum16(0, data))
}

/// UDP・TCP の疑似ヘッダの和
fn pseudo_sum(src: Ipv4, dst: Ipv4, proto: u8, len: usize) -> u32 {
    let acc = sum16(0, &src.0);
    let acc = sum16(acc, &dst.0);
    acc + proto as u32 + len as u32
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

// --- Ethernet ----------------------------------------------------------------

pub const ETH_HEADER: usize = 14;
/// Ethernet の最短のフレーム（FCS を除く）
pub const ETH_MIN_FRAME: usize = 60;
/// Ethernet の最長のフレーム（FCS を除く）
pub const ETH_MAX_FRAME: usize = 1514;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub struct EthFrame<'a> {
    pub dst: Mac,
    pub src: Mac,
    pub ethertype: u16,
    /// 中身（最短の長さに足りない分の詰め物を含むことがある）
    pub payload: &'a [u8],
}

pub fn parse_eth(f: &[u8]) -> Option<EthFrame<'_>> {
    if f.len() < ETH_HEADER {
        return None;
    }
    let mac = |at: usize| {
        let mut m = [0u8; 6];
        m.copy_from_slice(&f[at..at + 6]);
        Mac(m)
    };
    Some(EthFrame { dst: mac(0), src: mac(6), ethertype: be16(f, 12), payload: &f[ETH_HEADER..] })
}

/// フレームを作る（60 バイトに満たなければ 0 で埋める）
pub fn build_eth(dst: Mac, src: Mac, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity((ETH_HEADER + payload.len()).max(ETH_MIN_FRAME));
    f.extend_from_slice(&dst.0);
    f.extend_from_slice(&src.0);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    if f.len() < ETH_MIN_FRAME {
        f.resize(ETH_MIN_FRAME, 0);
    }
    f
}

// --- ARP ---------------------------------------------------------------------

pub const ARP_REQUEST: u16 = 1;
pub const ARP_REPLY: u16 = 2;
pub const ARP_LEN: usize = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arp {
    pub op: u16,
    pub sender_mac: Mac,
    pub sender_ip: Ipv4,
    pub target_mac: Mac,
    pub target_ip: Ipv4,
}

impl Arp {
    pub fn parse(p: &[u8]) -> Option<Arp> {
        // Ethernet（1）+ IPv4（0x0800）、アドレス長 6 / 4 のものだけ
        if p.len() < ARP_LEN || be16(p, 0) != 1 || be16(p, 2) != ETHERTYPE_IPV4 || p[4] != 6 || p[5] != 4 {
            return None;
        }
        let mac = |at: usize| {
            let mut m = [0u8; 6];
            m.copy_from_slice(&p[at..at + 6]);
            Mac(m)
        };
        let ip = |at: usize| Ipv4([p[at], p[at + 1], p[at + 2], p[at + 3]]);
        Some(Arp { op: be16(p, 6), sender_mac: mac(8), sender_ip: ip(14), target_mac: mac(18), target_ip: ip(24) })
    }

    pub fn to_bytes(&self) -> [u8; ARP_LEN] {
        let mut p = [0u8; ARP_LEN];
        p[0..2].copy_from_slice(&1u16.to_be_bytes());
        p[2..4].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        p[4] = 6;
        p[5] = 4;
        p[6..8].copy_from_slice(&self.op.to_be_bytes());
        p[8..14].copy_from_slice(&self.sender_mac.0);
        p[14..18].copy_from_slice(&self.sender_ip.0);
        p[18..24].copy_from_slice(&self.target_mac.0);
        p[24..28].copy_from_slice(&self.target_ip.0);
        p
    }
}

// --- IPv4 --------------------------------------------------------------------

pub const IP_HEADER: usize = 20;
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;
pub const DEFAULT_TTL: u8 = 64;

pub struct Ipv4Packet<'a> {
    pub src: Ipv4,
    pub dst: Ipv4,
    pub proto: u8,
    pub ttl: u8,
    pub ident: u16,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpError {
    Malformed,
    BadChecksum,
    /// フラグメント（組み立てはしない）
    Fragment,
}

/// IPv4 のパケットを読む。後ろの詰め物（Ethernet の最短の長さに合わせたもの）は全長で切る
pub fn parse_ipv4(p: &[u8]) -> Result<Ipv4Packet<'_>, IpError> {
    if p.len() < IP_HEADER || p[0] >> 4 != 4 {
        return Err(IpError::Malformed);
    }
    let ihl = (p[0] & 0x0F) as usize * 4;
    let total = be16(p, 2) as usize;
    if ihl < IP_HEADER || total < ihl || total > p.len() {
        return Err(IpError::Malformed);
    }
    if checksum(&p[..ihl]) != 0 {
        return Err(IpError::BadChecksum);
    }
    let frag = be16(p, 6);
    // MF（もっとある）か、フラグメントの位置が 0 でない
    if frag & 0x2000 != 0 || frag & 0x1FFF != 0 {
        return Err(IpError::Fragment);
    }
    Ok(Ipv4Packet {
        src: Ipv4([p[12], p[13], p[14], p[15]]),
        dst: Ipv4([p[16], p[17], p[18], p[19]]),
        proto: p[9],
        ttl: p[8],
        ident: be16(p, 4),
        payload: &p[ihl..total],
    })
}

/// IPv4 のパケットを作る（オプションなし。DF を立てる）
pub fn build_ipv4(src: Ipv4, dst: Ipv4, proto: u8, ttl: u8, ident: u16, payload: &[u8]) -> Vec<u8> {
    let total = IP_HEADER + payload.len();
    let mut p = Vec::with_capacity(total);
    p.push(0x45);
    p.push(0);
    p.extend_from_slice(&(total as u16).to_be_bytes());
    p.extend_from_slice(&ident.to_be_bytes());
    p.extend_from_slice(&0x4000u16.to_be_bytes()); // DF
    p.push(ttl);
    p.push(proto);
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(&src.0);
    p.extend_from_slice(&dst.0);
    let c = checksum(&p[..IP_HEADER]);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

// --- ICMP --------------------------------------------------------------------

pub const ICMP_ECHO_REPLY: u8 = 0;
pub const ICMP_DEST_UNREACHABLE: u8 = 3;
pub const ICMP_ECHO_REQUEST: u8 = 8;
/// 到達できない理由: ポートが閉じている
pub const ICMP_PORT_UNREACHABLE: u8 = 3;
pub const ICMP_HEADER: usize = 8;

pub struct Icmp<'a> {
    pub kind: u8,
    pub code: u8,
    /// エコーのときは識別子と通し番号
    pub rest: [u8; 4],
    pub data: &'a [u8],
}

impl Icmp<'_> {
    pub fn ident(&self) -> u16 {
        be16(&self.rest, 0)
    }

    pub fn seq(&self) -> u16 {
        be16(&self.rest, 2)
    }
}

pub fn parse_icmp(p: &[u8]) -> Option<Icmp<'_>> {
    if p.len() < ICMP_HEADER || checksum(p) != 0 {
        return None;
    }
    Some(Icmp { kind: p[0], code: p[1], rest: [p[4], p[5], p[6], p[7]], data: &p[ICMP_HEADER..] })
}

pub fn build_icmp(kind: u8, code: u8, rest: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(ICMP_HEADER + data.len());
    p.extend_from_slice(&[kind, code, 0, 0]);
    p.extend_from_slice(&rest);
    p.extend_from_slice(data);
    let c = checksum(&p);
    p[2..4].copy_from_slice(&c.to_be_bytes());
    p
}

/// エコー要求・応答の rest（識別子と通し番号）
pub fn echo_rest(ident: u16, seq: u16) -> [u8; 4] {
    let i = ident.to_be_bytes();
    let s = seq.to_be_bytes();
    [i[0], i[1], s[0], s[1]]
}

// --- UDP ---------------------------------------------------------------------

pub const UDP_HEADER: usize = 8;

pub struct Udp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: &'a [u8],
}

/// UDP を読む（チェックサムが 0 以外なら確かめる）
pub fn parse_udp(src: Ipv4, dst: Ipv4, p: &[u8]) -> Option<Udp<'_>> {
    if p.len() < UDP_HEADER {
        return None;
    }
    let len = be16(p, 4) as usize;
    if len < UDP_HEADER || len > p.len() {
        return None;
    }
    if be16(p, 6) != 0 && fold(sum16(pseudo_sum(src, dst, PROTO_UDP, len), &p[..len])) != 0 {
        return None;
    }
    Some(Udp { src_port: be16(p, 0), dst_port: be16(p, 2), payload: &p[UDP_HEADER..len] })
}

pub fn build_udp(src: Ipv4, dst: Ipv4, src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let len = UDP_HEADER + payload.len();
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(&src_port.to_be_bytes());
    p.extend_from_slice(&dst_port.to_be_bytes());
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(payload);
    let mut c = fold(sum16(pseudo_sum(src, dst, PROTO_UDP, len), &p));
    if c == 0 {
        c = 0xFFFF; // 0 は「チェックサムなし」の意味になるので
    }
    p[6..8].copy_from_slice(&c.to_be_bytes());
    p
}

// --- TCP ---------------------------------------------------------------------

pub const TCP_HEADER: usize = 20;
pub const TCP_FIN: u8 = 0x01;
pub const TCP_SYN: u8 = 0x02;
pub const TCP_RST: u8 = 0x04;
pub const TCP_PSH: u8 = 0x08;
pub const TCP_ACK: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TcpHeader {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// MSS オプション（SYN のときだけ）
    pub mss: Option<u16>,
}

pub struct Tcp<'a> {
    pub header: TcpHeader,
    pub payload: &'a [u8],
}

pub fn parse_tcp(src: Ipv4, dst: Ipv4, p: &[u8]) -> Option<Tcp<'_>> {
    if p.len() < TCP_HEADER {
        return None;
    }
    let off = (p[12] >> 4) as usize * 4;
    if off < TCP_HEADER || off > p.len() {
        return None;
    }
    if fold(sum16(pseudo_sum(src, dst, PROTO_TCP, p.len()), p)) != 0 {
        return None;
    }
    // オプション: MSS（種類 2）だけ読み、ほかは長さで飛ばす
    let mut mss = None;
    let mut i = TCP_HEADER;
    while i < off {
        match p[i] {
            0 => break,
            1 => i += 1,
            kind => {
                let len = *p.get(i + 1)? as usize;
                if len < 2 || i + len > off {
                    return None;
                }
                if kind == 2 && len == 4 {
                    mss = Some(be16(p, i + 2));
                }
                i += len;
            }
        }
    }
    Some(Tcp {
        header: TcpHeader {
            src_port: be16(p, 0),
            dst_port: be16(p, 2),
            seq: be32(p, 4),
            ack: be32(p, 8),
            flags: p[13] & 0x3F,
            window: be16(p, 14),
            mss,
        },
        payload: &p[off..],
    })
}

pub fn build_tcp(src: Ipv4, dst: Ipv4, h: &TcpHeader, payload: &[u8]) -> Vec<u8> {
    let opt_len = if h.mss.is_some() { 4 } else { 0 };
    let off = TCP_HEADER + opt_len;
    let mut p = Vec::with_capacity(off + payload.len());
    p.extend_from_slice(&h.src_port.to_be_bytes());
    p.extend_from_slice(&h.dst_port.to_be_bytes());
    p.extend_from_slice(&h.seq.to_be_bytes());
    p.extend_from_slice(&h.ack.to_be_bytes());
    p.push(((off / 4) as u8) << 4);
    p.push(h.flags);
    p.extend_from_slice(&h.window.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]); // チェックサム, 緊急ポインタ
    if let Some(mss) = h.mss {
        p.extend_from_slice(&[2, 4]);
        p.extend_from_slice(&mss.to_be_bytes());
    }
    p.extend_from_slice(payload);
    let c = fold(sum16(pseudo_sum(src, dst, PROTO_TCP, p.len()), &p));
    p[16..18].copy_from_slice(&c.to_be_bytes());
    p
}
