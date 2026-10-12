//! ホストでのテスト: 2 つのスタックを直結した回線（落とす・重ねる・入れ替える）で通信させる

use alloc::vec::Vec;
use std::collections::VecDeque;

use crate::stack::{IfConfig, NetError, Stack};
use crate::tcp::TcpState;
use crate::wire::{self, *};
use crate::{Ipv4, Mac, config, dhcp, dns, http, sntp};

const A_IP: Ipv4 = Ipv4::new(10, 0, 0, 1);
const B_IP: Ipv4 = Ipv4::new(10, 0, 0, 2);
const A_MAC: Mac = Mac([0x52, 0x54, 0, 0, 0, 1]);
const B_MAC: Mac = Mac([0x52, 0x54, 0, 0, 0, 2]);

fn stack(mac: Mac, ip: Ipv4, seed: u32) -> Stack {
    let mut s = Stack::new(mac, seed);
    s.set_static(IfConfig { addr: ip, prefix: 24, gateway: None, dns: None });
    s
}

/// 回線の癖
#[derive(Clone, Copy, Default)]
struct Faults {
    /// n 個ごとに 1 つ落とす（0 なら落とさない）
    drop_every: usize,
    /// n 個ごとに 1 つ二重に届ける
    dup_every: usize,
    /// n 個ごとに次のフレームと順番を入れ替える
    swap_every: usize,
    /// 1000 個あたりこの数をでたらめに落とす
    loss_permille: u32,
}

struct Net {
    a: Stack,
    b: Stack,
    now: u64,
    faults: Faults,
    count: usize,
    rng: u32,
    frames: Vec<Vec<u8>>,
}

impl Net {
    fn new(faults: Faults) -> Net {
        Net { a: stack(A_MAC, A_IP, 1), b: stack(B_MAC, B_IP, 2), now: 0, faults, count: 0, rng: 12345, frames: Vec::new() }
    }

    /// 回線を通す（癖を当てはめて）
    fn carry(&mut self, from_a: bool) {
        let mut q: VecDeque<Vec<u8>> = VecDeque::new();
        let src = if from_a { &mut self.a } else { &mut self.b };
        while let Some(f) = src.transmit() {
            self.frames.push(f.clone());
            self.count += 1;
            let n = self.count;
            let f2 = f.clone();
            if self.faults.drop_every != 0 && n % self.faults.drop_every == 0 {
                continue;
            }
            self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
            if (self.rng >> 16) % 1000 < self.faults.loss_permille {
                continue;
            }
            q.push_back(f);
            if self.faults.dup_every != 0 && n % self.faults.dup_every == 0 {
                q.push_back(f2);
            }
            if self.faults.swap_every != 0 && n % self.faults.swap_every == 0 && q.len() >= 2 {
                let l = q.len();
                q.swap(l - 1, l - 2);
            }
        }
        let dst = if from_a { &mut self.b } else { &mut self.a };
        for f in q {
            dst.receive(&f, self.now);
        }
    }

    fn step(&mut self) {
        self.now += 10;
        self.a.poll(self.now);
        self.b.poll(self.now);
        self.carry(true);
        self.carry(false);
    }

    fn run_until(&mut self, max_ms: u64, mut cond: impl FnMut(&mut Net) -> bool) -> bool {
        let end = self.now + max_ms;
        while self.now < end {
            if cond(self) {
                return true;
            }
            self.step();
        }
        cond(self)
    }
}

// --- 形式 ------------------------------------------------------------------------

#[test]
fn checksum_rfc1071_example() {
    // RFC 1071 の例: 0001 f203 f4f5 f6f7 の和は ddf2 → 補数 220d
    let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
    assert_eq!(checksum(&data), 0x220d);
    // 奇数の長さ
    assert_eq!(checksum(&[0xff]), !0xff00u16);
}

#[test]
fn ipv4_round_trip_and_rejects() {
    let p = build_ipv4(A_IP, B_IP, PROTO_UDP, 64, 7, b"hello");
    let q = parse_ipv4(&p).ok().unwrap();
    assert_eq!((q.src, q.dst, q.proto, q.ttl, q.ident, q.payload), (A_IP, B_IP, PROTO_UDP, 64, 7, &b"hello"[..]));
    // 後ろの詰め物は全長で切る
    let mut padded = p.clone();
    padded.extend_from_slice(&[0; 20]);
    assert_eq!(parse_ipv4(&padded).ok().unwrap().payload, b"hello");
    let mut bad = p.clone();
    bad[8] = 1; // TTL を変えるとチェックサムが合わない
    assert_eq!(parse_ipv4(&bad).err(), Some(IpError::BadChecksum));
    let mut frag = p.clone();
    frag[6] = 0x20; // MF
    let c = checksum(&{
        let mut h = frag[..20].to_vec();
        h[10] = 0;
        h[11] = 0;
        h
    });
    frag[10..12].copy_from_slice(&c.to_be_bytes());
    assert_eq!(parse_ipv4(&frag).err(), Some(IpError::Fragment));
}

#[test]
fn udp_and_tcp_checksums_verify() {
    let u = build_udp(A_IP, B_IP, 1000, 2000, b"abc");
    assert!(parse_udp(A_IP, B_IP, &u).is_some());
    assert!(parse_udp(A_IP, Ipv4::new(10, 0, 0, 3), &u).is_none()); // 疑似ヘッダが違う
    let h = TcpHeader { src_port: 1, dst_port: 2, seq: 3, ack: 4, flags: TCP_SYN, window: 100, mss: Some(1460) };
    let t = build_tcp(A_IP, B_IP, &h, b"xyz");
    let p = parse_tcp(A_IP, B_IP, &t).unwrap();
    assert_eq!(p.header, h);
    assert_eq!(p.payload, b"xyz");
    let mut bad = t.clone();
    bad[25] ^= 1;
    assert!(parse_tcp(A_IP, B_IP, &bad).is_none());
}

#[test]
fn ipv4_addr_helpers() {
    assert_eq!(Ipv4::parse("10.0.2.15"), Some(Ipv4::new(10, 0, 2, 15)));
    assert_eq!(Ipv4::parse("10.0.2"), None);
    assert_eq!(Ipv4::parse("10.0.2.256"), None);
    assert_eq!(Ipv4::parse("1.2.3.4.5"), None);
    assert_eq!(Ipv4::netmask(24), Ipv4::new(255, 255, 255, 0));
    assert_eq!(Ipv4::new(255, 255, 240, 0).prefix_len(), Some(20));
    assert_eq!(Ipv4::new(255, 0, 255, 0).prefix_len(), None);
    assert_eq!(std::format!("[{:>10}]", Ipv4::new(1, 2, 3, 4)), "[   1.2.3.4]");
    assert_eq!(std::format!("{}", A_MAC), "52:54:00:00:00:01");
}

// --- ARP・ICMP・UDP ------------------------------------------------------------------

#[test]
fn ping_resolves_arp_and_gets_reply() {
    let mut net = Net::new(Faults::default());
    let sock = net.a.icmp_open().unwrap();
    let mut msg = build_icmp(ICMP_ECHO_REQUEST, 0, echo_rest(0, 1), b"ping data");
    msg[2] = 0; // チェックサムはスタックが入れ直す
    net.a.icmp_send(sock, B_IP, &msg).unwrap();
    // 最初のフレームは ARP の問い合わせ
    let first = net.a.transmit().unwrap();
    let eth = parse_eth(&first).unwrap();
    assert_eq!((eth.dst, eth.ethertype), (Mac::BROADCAST, ETHERTYPE_ARP));
    let arp = Arp::parse(eth.payload).unwrap();
    assert_eq!((arp.op, arp.sender_ip, arp.target_ip), (ARP_REQUEST, A_IP, B_IP));
    net.b.receive(&first, 0);
    assert!(net.run_until(1000, |n| n.a.readiness(sock).readable));
    let (src, reply, ttl) = net.a.icmp_recv(sock).unwrap();
    let r = parse_icmp(&reply).unwrap();
    assert_eq!((src, r.kind, r.seq(), r.data, ttl), (B_IP, ICMP_ECHO_REPLY, 1, &b"ping data"[..], 64));
    assert_eq!(net.a.arp_lookup(B_IP), Some(B_MAC));
    assert_eq!(net.b.arp_lookup(A_IP), Some(A_MAC)); // 問い合わせてきた相手を覚える
}

#[test]
fn arp_gives_up_without_answer() {
    let mut a = stack(A_MAC, A_IP, 1);
    let sock = a.udp_open(0).unwrap();
    a.udp_send(sock, B_IP, 9, b"x").unwrap();
    let mut requests = 0;
    for t in 0..500 {
        a.poll(t * 10);
        while let Some(f) = a.transmit() {
            assert_eq!(parse_eth(&f).unwrap().ethertype, ETHERTYPE_ARP);
            requests += 1;
        }
    }
    assert_eq!(requests, 3);
    assert!(!a.arp_pending(B_IP));
    assert_eq!(a.next_deadline(), None);
}

#[test]
fn udp_exchange_and_port_unreachable() {
    let mut net = Net::new(Faults::default());
    let sa = net.a.udp_open(0).unwrap();
    let sb = net.b.udp_open(5000).unwrap();
    assert_eq!(net.b.udp_open(5000), Err(NetError::AddrInUse));
    net.a.udp_send(sa, B_IP, 5000, b"question").unwrap();
    assert!(net.run_until(1000, |n| n.b.readiness(sb).readable));
    let (from, port, data) = net.b.udp_recv(sb).unwrap();
    assert_eq!((from, data.as_slice()), (A_IP, &b"question"[..]));
    net.b.udp_send(sb, from, port, b"answer").unwrap();
    assert!(net.run_until(1000, |n| n.a.readiness(sa).readable));
    assert_eq!(net.a.udp_recv(sa).unwrap().2, b"answer");
    assert_eq!(net.a.udp_send(sa, B_IP, 1, &[0; 1500]), Err(NetError::TooBig));

    // 閉じたポート: ICMP の「ポートに届かない」が返る
    let before = net.frames.len();
    net.a.udp_send(sa, B_IP, 6000, b"nobody").unwrap();
    net.run_until(200, |_| false);
    let unreachable = net.frames[before..].iter().any(|f| {
        let e = parse_eth(f).unwrap();
        e.ethertype == ETHERTYPE_IPV4 && {
            let ip = parse_ipv4(e.payload).ok().unwrap();
            ip.proto == PROTO_ICMP && parse_icmp(ip.payload).is_some_and(|m| m.kind == ICMP_DEST_UNREACHABLE && m.code == 3)
        }
    });
    assert!(unreachable);
}

#[test]
fn not_configured_and_no_route() {
    let mut s = Stack::new(A_MAC, 1);
    let u = s.udp_open(0).unwrap();
    assert_eq!(s.udp_send(u, B_IP, 1, b"x"), Err(NetError::NotConfigured));
    assert_eq!(s.tcp_connect(B_IP, 80).err(), Some(NetError::NotConfigured));
    let mut s = stack(A_MAC, A_IP, 1);
    let u = s.udp_open(0).unwrap();
    assert_eq!(s.udp_send(u, Ipv4::new(8, 8, 8, 8), 53, b"x"), Err(NetError::NoRoute));
}

#[test]
fn gateway_is_used_for_other_networks() {
    let mut s = Stack::new(A_MAC, 1);
    s.set_static(IfConfig { addr: A_IP, prefix: 24, gateway: Some(B_IP), dns: None });
    let u = s.udp_open(0).unwrap();
    s.udp_send(u, Ipv4::new(192, 168, 1, 1), 53, b"x").unwrap();
    let f = s.transmit().unwrap();
    let arp = Arp::parse(parse_eth(&f).unwrap().payload).unwrap();
    assert_eq!(arp.target_ip, B_IP); // ゲートウェイの MAC を問い合わせる
}

// --- TCP ---------------------------------------------------------------------------

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

/// a から b へ len バイト送って、b が受け取ったものを返す。どちらも最後に閉じる
fn tcp_transfer(net: &mut Net, len: usize, max_ms: u64) -> Vec<u8> {
    let l = net.b.tcp_listen(80, 4).unwrap();
    let c = net.a.tcp_connect(B_IP, 80).unwrap();
    assert!(net.run_until(20_000, |n| n.a.readiness(c).connected), "connect");
    assert_eq!(net.a.tcp_state(c), Some(TcpState::Established));
    assert!(net.run_until(20_000, |n| n.b.readiness(l).readable), "accept");
    let (s, peer, _) = net.b.tcp_accept(l).unwrap();
    assert_eq!(peer, A_IP);
    let data = pattern(len);
    let mut sent = 0;
    let mut got = Vec::new();
    let mut buf = [0u8; 700];
    let mut closed = false;
    let ok = net.run_until(max_ms, |n| {
        while sent < data.len() {
            match n.a.tcp_send(c, &data[sent..]) {
                Ok(k) => sent += k,
                Err(NetError::WouldBlock) => break,
                Err(e) => panic!("send: {e:?}"),
            }
        }
        if sent == data.len() && !closed {
            n.a.close(c);
            closed = true;
        }
        loop {
            match n.b.tcp_recv(s, &mut buf) {
                Ok(0) => return true,
                Ok(k) => got.extend_from_slice(&buf[..k]),
                Err(NetError::WouldBlock) => return false,
                Err(e) => panic!("recv: {e:?}"),
            }
        }
    });
    if !ok {
        std::eprintln!("a: {:?}\nb: {:?}\nerr a={:?} b={:?} sent={sent}", n_list(&net.a), n_list(&net.b), net.a.tcp_error(c), net.b.tcp_error(s));
        for f in &net.frames[net.frames.len().saturating_sub(12)..] {
            std::eprintln!("{}", describe(f));
        }
    }
    assert!(ok, "transfer did not finish ({} of {len} bytes)", got.len());
    net.b.close(s);
    net.b.close(l);
    got
}

#[test]
fn tcp_transfer_100kb() {
    let mut net = Net::new(Faults::default());
    let got = tcp_transfer(&mut net, 100 * 1024, 60_000);
    assert!(got == pattern(100 * 1024));
    // 両方が閉じたら（TIME_WAIT の後）ソケットはすべて片付く
    assert!(net.run_until(10_000, |n| n.a.socket_list().is_empty() && n.b.socket_list().is_empty()));
}

#[test]
fn tcp_transfer_over_lossy_link() {
    // 決まった間隔で落とす（4 つに 1 つは厳しいが、時間をかければ届く）
    for (drop_every, limit) in [(4, 120_000), (7, 30_000), (10, 30_000)] {
        let mut net = Net::new(Faults { drop_every, ..Default::default() });
        let got = tcp_transfer(&mut net, 100 * 1024, limit);
        assert!(got == pattern(100 * 1024), "drop every {drop_every}");
    }
    // でたらめに 10%・20% 落とす
    for loss_permille in [100, 200] {
        let mut net = Net::new(Faults { loss_permille, ..Default::default() });
        let got = tcp_transfer(&mut net, 100 * 1024, 600_000);
        assert!(got == pattern(100 * 1024), "loss {loss_permille}/1000");
    }
}

#[test]
fn tcp_small_receive_window() {
    // 受信の窓を小さくしても届く（相手は窓の分ずつしか送らない）
    let mut net = Net::new(Faults::default());
    net.b.set_recv_window(4096);
    let before = net.frames.len();
    let got = tcp_transfer(&mut net, 30 * 1024, 30_000);
    assert!(got == pattern(30 * 1024));
    // b が知らせた窓は 4096 を超えない
    for f in &net.frames[before..] {
        let ip = parse_ipv4(parse_eth(f).unwrap().payload).ok();
        if let Some(ip) = ip.filter(|ip| ip.src == B_IP && ip.proto == PROTO_TCP) {
            assert!(parse_tcp(ip.src, ip.dst, ip.payload).unwrap().header.window <= 4096);
        }
    }
}

#[test]
fn tcp_transfer_with_duplicates_and_reordering() {
    let mut net = Net::new(Faults { dup_every: 5, swap_every: 4, ..Default::default() });
    let got = tcp_transfer(&mut net, 50 * 1024, 300_000);
    assert!(got == pattern(50 * 1024));
}

#[test]
fn tcp_connect_refused() {
    let mut net = Net::new(Faults::default());
    let c = net.a.tcp_connect(B_IP, 81).unwrap();
    assert!(net.run_until(5000, |n| n.a.readiness(c).connected));
    assert_eq!(net.a.tcp_error(c), Some(NetError::ConnRefused));
    let mut buf = [0u8; 4];
    assert_eq!(net.a.tcp_send(c, b"x"), Err(NetError::ConnRefused));
    assert_eq!(net.a.tcp_recv(c, &mut buf), Err(NetError::ConnRefused));
}

#[test]
fn tcp_connect_times_out() {
    let mut net = Net::new(Faults { drop_every: 1, ..Default::default() }); // 何も届かない
    let c = net.a.tcp_connect(B_IP, 80).unwrap();
    assert!(net.run_until(120_000, |n| n.a.readiness(c).connected));
    // ARP が答えないので SYN が出ないまま、再送の上限で諦める
    assert_eq!(net.a.tcp_error(c), Some(NetError::TimedOut));
}

#[test]
fn tcp_zero_window_then_reads() {
    let mut net = Net::new(Faults::default());
    let l = net.b.tcp_listen(80, 1).unwrap();
    let c = net.a.tcp_connect(B_IP, 80).unwrap();
    assert!(net.run_until(2000, |n| n.b.readiness(l).readable));
    let (s, _, _) = net.b.tcp_accept(l).unwrap();
    // 受け取る側が読まないので、受信の窓（16KB）と送信のバッファ（16KB）が一杯になる
    let data = pattern(40 * 1024);
    let mut sent = 0;
    net.run_until(5000, |n| {
        if let Ok(k) = n.a.tcp_send(c, &data[sent..]) {
            sent += k;
        }
        false
    });
    assert_eq!(sent, 32 * 1024);
    assert!(!net.a.readiness(c).writable);
    // 長く待っても切れない（窓をのぞき続ける。再送の上限で諦めるより十分長く）
    net.run_until(900_000, |_| false);
    assert_eq!(net.a.tcp_state(c), Some(TcpState::Established));
    // 読み始めると残りも届く
    let mut got = Vec::new();
    let mut buf = [0u8; 1000];
    assert!(net.run_until(60_000, |n| {
        if sent < data.len() {
            if let Ok(k) = n.a.tcp_send(c, &data[sent..]) {
                sent += k;
            }
        }
        while let Ok(k) = n.b.tcp_recv(s, &mut buf) {
            if k == 0 {
                break;
            }
            got.extend_from_slice(&buf[..k]);
        }
        got.len() == data.len()
    }));
    assert!(got == data);
}

#[test]
fn tcp_both_directions_and_half_close() {
    let mut net = Net::new(Faults::default());
    let l = net.b.tcp_listen(7, 1).unwrap();
    let c = net.a.tcp_connect(B_IP, 7).unwrap();
    assert!(net.run_until(2000, |n| n.b.readiness(l).readable));
    let (s, _, _) = net.b.tcp_accept(l).unwrap();
    net.a.tcp_send(c, b"GET /").unwrap();
    net.a.close(c); // 送り終えた（読むのは続ける）
    let mut buf = [0u8; 64];
    let mut req = Vec::new();
    assert!(net.run_until(2000, |n| match n.b.tcp_recv(s, &mut buf) {
        Ok(0) => true,
        Ok(k) => {
            req.extend_from_slice(&buf[..k]);
            false
        }
        Err(_) => false,
    }));
    assert_eq!(req, b"GET /");
    assert_eq!(net.b.tcp_state(s), Some(TcpState::CloseWait));
    // 閉じた側にも返事は届く（a のソケットは close したので読めないが、状態は進む）
    net.b.tcp_send(s, b"reply").unwrap();
    net.b.close(s);
    assert!(net.run_until(5000, |n| n.b.socket_list().iter().all(|i| i.state == "LISTEN")));
}

#[test]
fn tcp_accept_backlog_and_listener_close() {
    let mut net = Net::new(Faults::default());
    let l = net.b.tcp_listen(80, 2).unwrap();
    let c1 = net.a.tcp_connect(B_IP, 80).unwrap();
    let c2 = net.a.tcp_connect(B_IP, 80).unwrap();
    let c3 = net.a.tcp_connect(B_IP, 80).unwrap();
    net.run_until(500, |_| false);
    assert!(net.a.readiness(c1).connected && net.a.readiness(c2).connected);
    // 3 本目は待ち行列が一杯なので SYN が無視され、送り直しを待っている
    assert_eq!(net.a.tcp_state(c3), Some(TcpState::SynSent));
    let (s1, _, _) = net.b.tcp_accept(l).unwrap();
    assert!(net.run_until(5000, |n| n.a.readiness(c3).connected));
    // 待ち受けを閉じると、まだ accept していない接続は切れる
    net.b.close(l);
    net.run_until(500, |_| false);
    assert_eq!(net.a.tcp_error(c2), Some(NetError::ConnReset));
    assert_eq!(net.a.tcp_state(c1), Some(TcpState::Established));
    assert_eq!(net.b.tcp_state(s1), Some(TcpState::Established));
}

#[test]
fn tcp_loopback_in_one_stack() {
    let mut s = stack(A_MAC, A_IP, 1);
    let l = s.tcp_listen(8080, 1).unwrap();
    let c = s.tcp_connect(Ipv4::LOOPBACK, 8080).unwrap();
    let mut now = 0;
    let mut spin = |s: &mut Stack, cond: &mut dyn FnMut(&mut Stack) -> bool| {
        for _ in 0..100 {
            if cond(s) {
                return true;
            }
            now += 10;
            s.poll(now);
            assert!(s.transmit().is_none(), "loopback must not reach the card");
        }
        false
    };
    assert!(spin(&mut s, &mut |s| s.readiness(l).readable));
    let (srv, peer, _) = s.tcp_accept(l).unwrap();
    assert_eq!(peer, Ipv4::LOOPBACK);
    s.tcp_send(c, b"hello self").unwrap();
    let mut buf = [0u8; 32];
    let mut n = 0;
    assert!(spin(&mut s, &mut |s| {
        n = s.tcp_recv(srv, &mut buf).unwrap_or(0);
        n > 0
    }));
    assert_eq!(&buf[..n], b"hello self");
    // 自分のアドレス宛ても同じ
    let c2 = s.tcp_connect(A_IP, 8080).unwrap();
    assert!(spin(&mut s, &mut |s| s.readiness(c2).connected));
    assert_eq!(s.tcp_state(c2), Some(TcpState::Established));
}

#[test]
fn loopback_address_from_the_wire_is_dropped() {
    let mut b = stack(B_MAC, B_IP, 2);
    let msg = build_icmp(ICMP_ECHO_REQUEST, 0, echo_rest(1, 1), b"x");
    let ip = build_ipv4(A_IP, Ipv4::LOOPBACK, PROTO_ICMP, 64, 1, &msg);
    b.receive(&build_eth(B_MAC, A_MAC, ETHERTYPE_IPV4, &ip), 0);
    b.poll(10);
    assert!(b.transmit().is_none());
    assert_eq!(b.stats.rx_dropped, 1);
}

#[test]
fn tcp_rst_for_unknown_connection() {
    let mut b = stack(B_MAC, B_IP, 2);
    let h = TcpHeader { src_port: 1234, dst_port: 99, seq: 1000, ack: 0, flags: TCP_SYN, window: 1000, mss: None };
    let seg = build_tcp(A_IP, B_IP, &h, &[]);
    let ip = build_ipv4(A_IP, B_IP, PROTO_TCP, 64, 1, &seg);
    // 送り元の MAC を ARP の表に入れておく（問い合わせを省く）
    let arp = Arp { op: ARP_REQUEST, sender_mac: A_MAC, sender_ip: A_IP, target_mac: Mac::ZERO, target_ip: B_IP };
    b.receive(&build_eth(B_MAC, A_MAC, ETHERTYPE_ARP, &arp.to_bytes()), 0);
    while b.transmit().is_some() {}
    b.receive(&build_eth(B_MAC, A_MAC, ETHERTYPE_IPV4, &ip), 0);
    let f = b.transmit().unwrap();
    let p = parse_ipv4(parse_eth(&f).unwrap().payload).ok().unwrap();
    let t = parse_tcp(B_IP, A_IP, p.payload).unwrap();
    assert_eq!(t.header.flags, TCP_RST | TCP_ACK);
    assert_eq!(t.header.ack, 1001);
}

// --- DHCP ---------------------------------------------------------------------------

#[test]
fn dhcp_gets_address() {
    let mut s = Stack::new(A_MAC, 5);
    s.start_dhcp();
    let server = Ipv4::new(10, 0, 2, 2);
    let mut now = 0;
    let mut configured = None;
    for _ in 0..1000 {
        now += 10;
        s.poll(now);
        while let Some(f) = s.transmit() {
            let e = parse_eth(&f).unwrap();
            assert_eq!(e.dst, Mac::BROADCAST);
            let ip = parse_ipv4(e.payload).ok().unwrap();
            let u = parse_udp(ip.src, ip.dst, ip.payload).unwrap();
            assert_eq!((u.src_port, u.dst_port), (68, 67));
            let m = dhcp::parse(u.payload).unwrap();
            let kind = match m.kind {
                dhcp::DHCPDISCOVER => dhcp::DHCPOFFER,
                dhcp::DHCPREQUEST => {
                    assert_eq!(m.requested, Some(Ipv4::new(10, 0, 2, 15)));
                    dhcp::DHCPACK
                }
                k => panic!("unexpected DHCP message {k}"),
            };
            let reply = dhcp::build_reply(
                kind,
                m.xid,
                A_MAC,
                Ipv4::new(10, 0, 2, 15),
                server,
                Ipv4::new(255, 255, 255, 0),
                server,
                Ipv4::new(10, 0, 2, 3),
            );
            let udp = build_udp(server, Ipv4::BROADCAST, 67, 68, &reply);
            let pkt = build_ipv4(server, Ipv4::BROADCAST, PROTO_UDP, 64, 1, &udp);
            s.receive(&build_eth(Mac::BROADCAST, B_MAC, ETHERTYPE_IPV4, &pkt), now);
        }
        if let Some(c) = s.take_config_event() {
            configured = Some(c);
            break;
        }
    }
    let c = configured.expect("no address");
    assert_eq!(c, IfConfig { addr: Ipv4::new(10, 0, 2, 15), prefix: 24, gateway: Some(server), dns: Some(Ipv4::new(10, 0, 2, 3)) });
    // 設定ファイルの値が優先される
    s.set_overrides(None, Some(Ipv4::new(10, 0, 2, 50)));
    assert_eq!(s.config().unwrap().dns, Some(Ipv4::new(10, 0, 2, 50)));
}

#[test]
fn dhcp_retries_with_backoff() {
    let mut c = dhcp::Client::new(A_MAC, 1);
    let mut sent = Vec::new();
    for t in (0..40_000).step_by(10) {
        if c.poll(t).is_some() {
            sent.push(t);
        }
    }
    assert_eq!(sent, [0, 1000, 3000, 7000, 15_000, 31_000]);
}

// --- DNS・SNTP・HTTP・設定 ----------------------------------------------------------------

#[test]
fn dns_query_and_response() {
    let q = dns::build_query(0x1234, "example.com").unwrap();
    assert_eq!(&q[12..25], b"\x07example\x03com\x00");
    assert_eq!(dns::parse_query(&q).unwrap(), (0x1234, std::string::String::from("example.com")));
    let r = dns::build_response(&q, Some(Ipv4::new(93, 184, 216, 34))).unwrap();
    assert_eq!(dns::parse_response(&r, 0x1234), Ok(alloc::vec![Ipv4::new(93, 184, 216, 34)]));
    assert_eq!(dns::parse_response(&r, 0x9999), Err(dns::DnsError::WrongId));
    let nx = dns::build_response(&q, None).unwrap();
    assert_eq!(dns::parse_response(&nx, 0x1234), Err(dns::DnsError::NotFound));
    assert_eq!(dns::build_query(1, "bad name").err(), Some(dns::DnsError::BadName));
    assert_eq!(dns::build_query(1, "").err(), Some(dns::DnsError::BadName));
    assert!(dns::parse_response(&r[..20], 0x1234).is_err());
}

#[test]
fn dns_response_with_cname_chain() {
    // www.example.com CNAME example.com, example.com A 1.2.3.4（名前は圧縮）
    let mut r = alloc::vec![0x00, 0x07, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0];
    r.extend_from_slice(b"\x03www\x07example\x03com\x00\x00\x01\x00\x01");
    r.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xC0, 16]);
    r.extend_from_slice(&[0xC0, 16, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
    assert_eq!(dns::parse_response(&r, 7), Ok(alloc::vec![Ipv4::new(1, 2, 3, 4)]));
    let mut name = std::string::String::new();
    dns::read_name(&r, 12 + 21 + 12, &mut name).unwrap();
    assert_eq!(name, "example.com");
}

#[test]
fn sntp_round_trip() {
    let req = sntp::build_request(0xABCD);
    // 2026-10-12 00:00:00 UTC
    let t = 1_791_763_200;
    let resp = sntp::build_response(&req, t).unwrap();
    assert_eq!(sntp::parse_response(&resp, 0xABCD), Ok(t));
    assert_eq!(sntp::parse_response(&resp, 1), Err(sntp::SntpError::WrongReply));
    let mut kod = resp;
    kod[1] = 0;
    assert_eq!(sntp::parse_response(&kod, 0xABCD), Err(sntp::SntpError::Unsynchronized));
}

#[test]
fn http_url_and_heads() {
    let u = http::parse_url("http://10.0.2.2:8080/dir/a.txt").unwrap();
    assert_eq!((u.host, u.port, u.path), ("10.0.2.2", 8080, "/dir/a.txt"));
    let u = http::parse_url("http://example.com").unwrap();
    assert_eq!((u.host, u.port, u.path), ("example.com", 80, "/"));
    assert!(http::parse_url("https://x/").is_err());
    assert!(http::parse_url("http://:80/").is_err());
    let req = http::build_get(&u);
    assert!(req.starts_with(b"GET / HTTP/1.0\r\nHost: example.com\r\n"));

    let resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\ncontent-length: 5\r\n\r\nhello";
    assert_eq!(http::parse_response_head(&resp[..20]), Ok(None));
    let (h, end) = http::parse_response_head(resp).unwrap().unwrap();
    assert_eq!((h.status, h.content_length), (200, Some(5)));
    assert_eq!(http::header(&resp[..end], "CONTENT-TYPE"), Some("text/plain"));
    assert_eq!(http::header(&resp[..end], "Location"), None);
    let moved = b"HTTP/1.1 301 Moved Permanently\r\nLocation: https://github.com/\r\n\r\n";
    assert_eq!(http::header(moved, "location"), Some("https://github.com/"));
    assert_eq!(http::status_text(301), "Moved Permanently");
    assert_eq!(&resp[end..], b"hello");

    let (r, _) = http::parse_request_head(b"GET /DOCS/READ%20ME.TXT?x=1 HTTP/1.0\r\nHost: a\r\n\r\n").unwrap().unwrap();
    assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/DOCS/READ ME.TXT"));
    assert!(http::parse_request_head(b"GET nopath HTTP/1.0\r\n\r\n").is_err());
    assert_eq!(http::content_type("/A/B.TXT"), "text/plain");
    let head = http::build_response_head(404, "text/plain", 9);
    assert!(head.starts_with(b"HTTP/1.0 404 Not Found\r\n"));
    assert!(std::str::from_utf8(&head).unwrap().contains("Content-Length: 9\r\n"));
}

#[test]
fn boot_config() {
    let mut bad = Vec::new();
    let s = config::parse(
        "# comment\nvideo=640x480\nnet=ne2000,0x320,9  # card\nip=10.0.2.15/24\ngw=10.0.2.2\ndns=10.0.2.3\nntp=1.2.3.4\ntz=+09:00\nip=oops\n",
        |l| bad.push(l.0),
    );
    assert_eq!(s.device, config::Device::Ne2000 { io: 0x320, irq: 9 });
    assert_eq!(s.static_ip, Some((Ipv4::new(10, 0, 2, 15), 24)));
    assert_eq!((s.gateway, s.dns, s.ntp), (Some(Ipv4::new(10, 0, 2, 2)), Some(Ipv4::new(10, 0, 2, 3)), Some(Ipv4::new(1, 2, 3, 4))));
    assert_eq!(s.tz_minutes, 540);
    assert_eq!(bad, ["ip=oops"]);
    let d = config::parse("", |_| {});
    assert_eq!(d, config::NetSettings::default());
    assert_eq!(config::parse("net=none\ntz=-5:30", |_| {}).device, config::Device::None);
    assert_eq!(config::parse("tz=-5:30", |_| {}).tz_minutes, -330);
    assert_eq!(config::parse("net=ne2000", |_| {}).device, config::DEFAULT_NE2000);
    assert_eq!(config::parse("net=auto", |_| {}).device, config::Device::Auto);
    assert_eq!(d.device, config::Device::Auto);
}

#[test]
fn icmp_rejects_non_echo() {
    let mut s = stack(A_MAC, A_IP, 1);
    let i = s.icmp_open().unwrap();
    assert_eq!(s.icmp_send(i, B_IP, &[0; 4]), Err(NetError::InvalidArgument));
    let msg = wire::build_icmp(ICMP_ECHO_REPLY, 0, [0; 4], b"");
    assert_eq!(s.icmp_send(i, B_IP, &msg), Err(NetError::InvalidArgument));
}

fn n_list(s: &Stack) -> Vec<(usize, &'static str, usize, usize)> {
    s.socket_list().iter().map(|i| (i.id, i.state, i.recv_queued, i.send_queued)).collect()
}

fn describe(f: &[u8]) -> std::string::String {
    let e = parse_eth(f).unwrap();
    if e.ethertype != ETHERTYPE_IPV4 {
        return std::format!("eth {:x}", e.ethertype);
    }
    let ip = parse_ipv4(e.payload).ok().unwrap();
    match parse_tcp(ip.src, ip.dst, ip.payload) {
        Some(t) => std::format!(
            "{} -> {} seq={} ack={} flags={:02x} win={} len={}",
            ip.src, ip.dst, t.header.seq, t.header.ack, t.header.flags, t.header.window, t.payload.len()
        ),
        None => std::format!("ip proto {}", ip.proto),
    }
}

