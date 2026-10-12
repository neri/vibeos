//! DHCP クライアント（RFC 2131 の最小限）
//!
//! DISCOVER → OFFER → REQUEST → ACK でアドレスをもらう。返事が無ければ間隔を倍にしながら送り直す
//! （最大 16 秒）。リース期間の半分が過ぎたら、もう一度 REQUEST からやり直して延長する。
//! 送り先はいつもブロードキャスト（自分のアドレスが決まる前から使えるように）。

use alloc::vec::Vec;

use crate::{Ipv4, Mac};

pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;

const MAGIC: [u8; 4] = [99, 130, 83, 99];
const OP_REQUEST: u8 = 1;
const OP_REPLY: u8 = 2;

const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAM_LIST: u8 = 55;
const OPT_END: u8 = 255;

pub const DHCPDISCOVER: u8 = 1;
pub const DHCPOFFER: u8 = 2;
pub const DHCPREQUEST: u8 = 3;
pub const DHCPACK: u8 = 5;
pub const DHCPNAK: u8 = 6;

const RETRY_FIRST_MS: u64 = 1000;
const RETRY_MAX_MS: u64 = 16_000;

/// サーバからもらった設定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub addr: Ipv4,
    pub netmask: Ipv4,
    pub router: Option<Ipv4>,
    pub dns: Option<Ipv4>,
    pub server: Ipv4,
    /// 秒（無ければ 1 日とみなす）
    pub lease_secs: u32,
}

/// 届いたメッセージを読んだもの
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub op: u8,
    pub xid: u32,
    pub kind: u8,
    pub yiaddr: Ipv4,
    pub chaddr: Mac,
    pub netmask: Option<Ipv4>,
    pub router: Option<Ipv4>,
    pub dns: Option<Ipv4>,
    pub server: Option<Ipv4>,
    pub requested: Option<Ipv4>,
    pub lease_secs: Option<u32>,
}

fn ip_at(b: &[u8], at: usize) -> Ipv4 {
    Ipv4([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// DHCP のメッセージを読む（オプションのうち使うものだけ）
pub fn parse(p: &[u8]) -> Option<Message> {
    if p.len() < 240 || p[1] != 1 || p[2] != 6 || p[236..240] != MAGIC {
        return None;
    }
    let mut chaddr = [0u8; 6];
    chaddr.copy_from_slice(&p[28..34]);
    let mut m = Message {
        op: p[0],
        xid: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
        kind: 0,
        yiaddr: ip_at(p, 16),
        chaddr: Mac(chaddr),
        netmask: None,
        router: None,
        dns: None,
        server: None,
        requested: None,
        lease_secs: None,
    };
    let mut i = 240;
    while i < p.len() {
        let code = p[i];
        if code == OPT_END {
            break;
        }
        if code == OPT_PAD {
            i += 1;
            continue;
        }
        let len = *p.get(i + 1)? as usize;
        let v = p.get(i + 2..i + 2 + len)?;
        match code {
            OPT_MESSAGE_TYPE if len >= 1 => m.kind = v[0],
            OPT_SUBNET_MASK if len >= 4 => m.netmask = Some(ip_at(v, 0)),
            OPT_ROUTER if len >= 4 => m.router = Some(ip_at(v, 0)),
            OPT_DNS if len >= 4 => m.dns = Some(ip_at(v, 0)),
            OPT_SERVER_ID if len >= 4 => m.server = Some(ip_at(v, 0)),
            OPT_REQUESTED_IP if len >= 4 => m.requested = Some(ip_at(v, 0)),
            OPT_LEASE_TIME if len >= 4 => m.lease_secs = Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]])),
            _ => {}
        }
        i += 2 + len;
    }
    (m.kind != 0).then_some(m)
}

/// クライアントのメッセージを作る（DISCOVER・REQUEST）
pub fn build_request(kind: u8, xid: u32, mac: Mac, requested: Option<Ipv4>, server: Option<Ipv4>) -> Vec<u8> {
    let mut p = alloc::vec![0u8; 240];
    p[0] = OP_REQUEST;
    p[1] = 1; // Ethernet
    p[2] = 6;
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[10] = 0x80; // 返事はブロードキャストで（アドレスが決まる前は受け取れないことがあるため）
    p[28..34].copy_from_slice(&mac.0);
    p[236..240].copy_from_slice(&MAGIC);
    p.extend_from_slice(&[OPT_MESSAGE_TYPE, 1, kind]);
    if let Some(ip) = requested {
        p.extend_from_slice(&[OPT_REQUESTED_IP, 4]);
        p.extend_from_slice(&ip.0);
    }
    if let Some(ip) = server {
        p.extend_from_slice(&[OPT_SERVER_ID, 4]);
        p.extend_from_slice(&ip.0);
    }
    p.extend_from_slice(&[OPT_PARAM_LIST, 4, OPT_SUBNET_MASK, OPT_ROUTER, OPT_DNS, OPT_LEASE_TIME]);
    p.push(OPT_END);
    // BOOTP の最短の長さ（300 バイト）に満たないと受け付けないサーバがある
    if p.len() < 300 {
        p.resize(300, 0);
    }
    p
}

/// サーバの返事を作る（テスト用）
#[allow(clippy::too_many_arguments)]
pub fn build_reply(kind: u8, xid: u32, mac: Mac, yiaddr: Ipv4, server: Ipv4, netmask: Ipv4, router: Ipv4, dns: Ipv4) -> Vec<u8> {
    let mut p = alloc::vec![0u8; 240];
    p[0] = OP_REPLY;
    p[1] = 1;
    p[2] = 6;
    p[4..8].copy_from_slice(&xid.to_be_bytes());
    p[16..20].copy_from_slice(&yiaddr.0);
    p[20..24].copy_from_slice(&server.0);
    p[28..34].copy_from_slice(&mac.0);
    p[236..240].copy_from_slice(&MAGIC);
    p.extend_from_slice(&[OPT_MESSAGE_TYPE, 1, kind]);
    for (code, ip) in [(OPT_SERVER_ID, server), (OPT_SUBNET_MASK, netmask), (OPT_ROUTER, router), (OPT_DNS, dns)] {
        p.extend_from_slice(&[code, 4]);
        p.extend_from_slice(&ip.0);
    }
    p.extend_from_slice(&[OPT_LEASE_TIME, 4]);
    p.extend_from_slice(&86400u32.to_be_bytes());
    p.push(OPT_END);
    p
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// DISCOVER を送って OFFER を待つ
    Selecting,
    /// REQUEST を送って ACK を待つ
    Requesting { addr: Ipv4, server: Ipv4 },
    /// アドレスをもらった。renew_at に延長を始める
    Bound { renew_at: u64 },
}

pub struct Client {
    mac: Mac,
    xid: u32,
    state: State,
    next_send: u64,
    interval: u64,
    /// 今持っているリース
    pub lease: Option<Lease>,
}

impl Client {
    pub fn new(mac: Mac, xid: u32) -> Client {
        Client { mac, xid, state: State::Selecting, next_send: 0, interval: RETRY_FIRST_MS, lease: None }
    }

    /// 送る時刻になっていれば、送るメッセージ（UDP の中身）を返す
    pub fn poll(&mut self, now: u64) -> Option<Vec<u8>> {
        if let State::Bound { renew_at } = self.state {
            if now < renew_at {
                return None;
            }
            // 延長: 今のアドレスのまま REQUEST からやり直す
            let lease = self.lease?;
            self.xid = self.xid.wrapping_add(1);
            self.state = State::Requesting { addr: lease.addr, server: lease.server };
            self.next_send = now;
            self.interval = RETRY_FIRST_MS;
        }
        if now < self.next_send {
            return None;
        }
        self.next_send = now + self.interval;
        self.interval = (self.interval * 2).min(RETRY_MAX_MS);
        Some(match self.state {
            State::Selecting => build_request(DHCPDISCOVER, self.xid, self.mac, None, None),
            State::Requesting { addr, server } => build_request(DHCPREQUEST, self.xid, self.mac, Some(addr), Some(server)),
            State::Bound { .. } => return None,
        })
    }

    /// 次に何かする時刻
    pub fn next_deadline(&self) -> u64 {
        match self.state {
            State::Bound { renew_at } => renew_at,
            _ => self.next_send,
        }
    }

    /// サーバの返事を受け取る。新しくアドレスが決まったら（延長を含む）そのリースを返す
    pub fn receive(&mut self, payload: &[u8], now: u64) -> Option<Lease> {
        let m = parse(payload)?;
        if m.op != OP_REPLY || m.xid != self.xid || m.chaddr != self.mac {
            return None;
        }
        match (self.state, m.kind) {
            (State::Selecting, DHCPOFFER) => {
                let server = m.server?;
                self.state = State::Requesting { addr: m.yiaddr, server };
                self.next_send = now;
                self.interval = RETRY_FIRST_MS;
                None
            }
            (State::Requesting { addr, server }, DHCPACK) if m.yiaddr == addr => {
                let lease = Lease {
                    addr,
                    netmask: m.netmask.unwrap_or(Ipv4::new(255, 255, 255, 0)),
                    router: m.router,
                    dns: m.dns,
                    server: m.server.unwrap_or(server),
                    lease_secs: m.lease_secs.unwrap_or(86400),
                };
                self.lease = Some(lease);
                let half = (lease.lease_secs as u64 * 1000 / 2).max(10_000);
                self.state = State::Bound { renew_at: now + half };
                Some(lease)
            }
            (State::Requesting { .. }, DHCPNAK) => {
                // 断られた: 最初からやり直す
                self.lease = None;
                self.xid = self.xid.wrapping_add(1);
                self.state = State::Selecting;
                self.next_send = now;
                self.interval = RETRY_FIRST_MS;
                None
            }
            _ => None,
        }
    }
}
