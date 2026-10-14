//! ネットワークのテスト（シナリオの `net` など）
//!
//! QEMU の中にハブを作り、そこへ 3 つをつなぐ:
//! - ゲストのカード（既定は NE2000, I/O 0x300, IRQ 10）
//! - slirp（`restrict=on`: ゲストから外（インターネット・ホストの他のポート）へは出られない。
//!   DHCP・ゲートウェイ 10.0.2.2 への ping・明示した転送（guestfwd / hostfwd）だけが使える）
//! - このテストランナー（UNIX ドメインのデータグラムで Ethernet のフレームをやり取りする）。
//!   ハブを流れるフレームをすべて記録し（OUT/NAME.pcap）、10.0.2.50 の「偽のホスト」として
//!   ARP・ping・DNS・NTP に答える
//!
//! HTTP のサーバ（serve-http）は slirp の guestfwd で 10.0.2.100:80 への接続ごとにこのプログラムを
//! `http-serve DIR` として起動する（標準入出力が接続）。ゲストの httpd へはホストから hostfwd で
//! 127.0.0.1 の空いているポートを通してつなぐ。
//!
//! パケットの解析はゲストの vibe-net とは別に書いてある（同じ間違いを見逃さないように）。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::Result;

pub const FAKE_IP: [u8; 4] = [10, 0, 2, 50];
const FAKE_MAC: [u8; 6] = [0x02, 0x00, 0x0a, 0x00, 0x02, 0x32];
/// ゲストから見た HTTP のサーバ（guestfwd）
pub const WWW_IP: &str = "10.0.2.100";

/// シナリオで決めたネットワークの設定（起動前に集める）
#[derive(Default, Clone)]
pub struct NetSetup {
    /// QEMU の -device（netdev= は足す）
    pub device: String,
    pub dns: HashMap<String, [u8; 4]>,
    /// NTP で返す時刻（1970 年からの秒, UTC）
    pub ntp: Option<u64>,
    /// guestfwd で HTTP を返すディレクトリ
    pub www: Option<PathBuf>,
    /// ゲストの 80 番へのポート転送を使う
    pub hostfwd: bool,
    /// slirp を restrict=on にしない（ホストの LAN へ出られる。net-lan）
    pub lan: bool,
}

/// 動いている偽のホスト
pub struct FakeNet {
    pub frames: Arc<Mutex<Vec<(SystemTime, Vec<u8>)>>>,
    sock: Arc<UnixDatagram>,
    stop: Arc<AtomicBool>,
    our_sock: PathBuf,
    qemu_sock: PathBuf,
    /// ゲストの 80 番につながるホストのポート
    pub http_port: Option<u16>,
}

impl FakeNet {
    /// 偽のホストを動かし、QEMU に渡す引数を返す
    pub fn start(setup: &NetSetup, tag: &str) -> Result<(FakeNet, Vec<String>)> {
        let dir = std::env::temp_dir();
        let our_sock = dir.join(format!("vibe-run-{tag}.net"));
        let qemu_sock = dir.join(format!("vibe-run-{tag}.qemu-net"));
        let _ = fs::remove_file(&our_sock);
        let _ = fs::remove_file(&qemu_sock);
        let sock = UnixDatagram::bind(&our_sock).map_err(|e| format!("bind {}: {e}", our_sock.display()))?;
        sock.set_read_timeout(Some(Duration::from_millis(100))).unwrap();

        let http_port = if setup.hostfwd {
            // 空いているポートを探す（すぐ手放して QEMU に使わせる）
            let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
            Some(l.local_addr().unwrap().port())
        } else {
            None
        };

        let mut user = String::from(if setup.lan { "user,id=slirp0" } else { "user,id=slirp0,restrict=on" });
        if let Some(p) = http_port {
            user.push_str(&format!(",hostfwd=tcp:127.0.0.1:{p}-:80"));
        }
        if let Some(www) = &setup.www {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let abs = std::env::current_dir().unwrap().join(www);
            for p in [&exe, &abs] {
                if p.to_string_lossy().contains(',') {
                    return Err(format!("path must not contain ',': {}", p.display()));
                }
            }
            user.push_str(&format!(",guestfwd=tcp:{WWW_IP}:80-cmd:{} http-serve {}", exe.display(), abs.display()));
        }
        let args = vec![
            "-netdev".into(),
            "hubport,id=nic0,hubid=0".into(),
            "-device".into(),
            format!("{},netdev=nic0", setup.device),
            "-netdev".into(),
            user,
            "-netdev".into(),
            "hubport,id=hub1,hubid=0,netdev=slirp0".into(),
            "-netdev".into(),
            format!(
                "dgram,id=fake0,local.type=unix,local.path={},remote.type=unix,remote.path={}",
                qemu_sock.display(),
                our_sock.display()
            ),
            "-netdev".into(),
            "hubport,id=hub2,hubid=0,netdev=fake0".into(),
        ];

        let frames = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let sock = Arc::new(sock);
        let host = Host { setup: setup.clone(), qemu_sock: qemu_sock.clone() };
        let (f, s, so) = (frames.clone(), stop.clone(), sock.clone());
        thread::spawn(move || host.run(&so, f, s));
        Ok((FakeNet { frames, sock, stop, our_sock, qemu_sock, http_port }, args))
    }

    /// 止めて、記録したフレームを pcap に書く
    pub fn finish(&self, pcap: &Path) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = fs::remove_file(&self.our_sock);
        let _ = fs::remove_file(&self.qemu_sock);
        let _ = write_pcap(pcap, &self.frames.lock().unwrap());
    }

    /// 偽のホストからゲストへ送る: `arp IP`（ARP の問い合わせ）か `ping IP`（エコー要求）
    pub fn send(&self, spec: &str) -> Result<()> {
        let (kind, ip) = spec.split_once(' ').ok_or("expected arp|ping IP")?;
        let ip: Vec<u8> = ip.trim().split('.').map(|x| x.parse().ok()).collect::<Option<_>>().unwrap_or_default();
        let ip: [u8; 4] = ip.try_into().map_err(|_| "bad address")?;
        let frame = match kind {
            "arp" => {
                let mut a = vec![0, 1, 8, 0, 6, 4, 0, 1];
                a.extend_from_slice(&FAKE_MAC);
                a.extend_from_slice(&FAKE_IP);
                a.extend_from_slice(&[0; 6]);
                a.extend_from_slice(&ip);
                eth([0xFF; 6], 0x0806, &a)
            }
            "ping" => {
                // 宛先の MAC は、これまでに見たそのアドレスからのフレームの送り元
                let mac = self.mac_of(ip).ok_or("guest MAC not seen yet")?;
                let mut m = vec![8, 0, 0, 0, 0x12, 0x34, 0, 1];
                m.extend_from_slice(b"vibe-run ping");
                let c = checksum(&m);
                m[2..4].copy_from_slice(&c.to_be_bytes());
                eth(mac, 0x0800, &ipv4(FAKE_IP, ip, 1, &m))
            }
            _ => return Err("expected arp|ping IP".into()),
        };
        self.frames.lock().unwrap().push((SystemTime::now(), frame.clone()));
        self.sock.send_to(&frame, &self.qemu_sock).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn mac_of(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        let frames = self.frames.lock().unwrap();
        frames.iter().rev().find_map(|(_, f)| {
            let src_ip = match be16(f, 12) {
                0x0806 if f.len() >= 42 => &f[28..32],
                0x0800 if f.len() >= 34 => &f[26..30],
                _ => return None,
            };
            (src_ip == ip).then(|| f[6..12].try_into().unwrap())
        })
    }

    /// ゲスト（MAC 52:54:00:12:34:56）が送った IPv4 のパケットの宛先がすべて LAN（プライベート・
    /// リンクローカル・ブロードキャスト）の中
    pub fn expect_lan_only(&self) -> Result<()> {
        let frames = self.frames.lock().unwrap();
        for (_, f) in frames.iter() {
            if f.len() < 34 || f[6..12] != [0x52, 0x54, 0, 0x12, 0x34, 0x56] || be16(f, 12) != 0x0800 {
                continue;
            }
            let d = &f[30..34];
            let lan = matches!(d, [10, ..] | [192, 168, ..] | [169, 254, ..] | [127, ..] | [255, 255, 255, 255])
                || (d[0] == 172 && d[1] & 0xF0 == 16);
            if !lan {
                return Err(format!("guest sent a packet to {} (outside the LAN)", ip_str(d)));
            }
        }
        Ok(())
    }

    /// 記録したフレームに、説明に合うものがあるか
    pub fn expect_packet(&self, spec: &str) -> Result<()> {
        let words: Vec<&str> = spec.split_whitespace().collect();
        let [kind, src, dst] = words[..] else {
            return Err("expected KIND SRC DST".into());
        };
        let frames = self.frames.lock().unwrap();
        let mut seen = Vec::new();
        for (_, f) in frames.iter() {
            match describe(f) {
                Ok(p) => {
                    if p.kind == kind && (src == "*" || p.src == src) && (dst == "*" || p.dst == dst) {
                        return Ok(());
                    }
                    seen.push(format!("{} {} {}", p.kind, p.src, p.dst));
                }
                // 壊れたフレームがあればそれ自体が失敗
                Err(e) => return Err(format!("malformed frame: {e}: {}", hex(f))),
            }
        }
        seen.dedup();
        Err(format!("no `{spec}` in {} frames (seen: {})", frames.len(), seen.join(", ")))
    }
}

// --- 偽のホスト（10.0.2.50） ----------------------------------------------------------

struct Host {
    setup: NetSetup,
    qemu_sock: PathBuf,
}

impl Host {
    fn run(self, sock: &UnixDatagram, frames: Arc<Mutex<Vec<(SystemTime, Vec<u8>)>>>, stop: Arc<AtomicBool>) {
        let mut buf = vec![0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            let n = match sock.recv(&mut buf) {
                Ok(n) => n,
                Err(_) => continue, // 時間切れ（止める印を見るため）
            };
            let frame = buf[..n].to_vec();
            frames.lock().unwrap().push((SystemTime::now(), frame.clone()));
            if let Some(reply) = self.answer(&frame) {
                frames.lock().unwrap().push((SystemTime::now(), reply.clone()));
                let _ = sock.send_to(&reply, &self.qemu_sock);
            }
        }
    }

    fn answer(&self, f: &[u8]) -> Option<Vec<u8>> {
        if f.len() < 14 {
            return None;
        }
        let src_mac: [u8; 6] = f[6..12].try_into().unwrap();
        match be16(f, 12) {
            0x0806 => {
                let a = &f[14..];
                // ARP の問い合わせ（Ethernet / IPv4）で、宛先が偽のホスト
                if a.len() >= 28 && be16(a, 6) == 1 && a[24..28] == FAKE_IP {
                    let mut r = Vec::with_capacity(28);
                    r.extend_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
                    r.extend_from_slice(&FAKE_MAC);
                    r.extend_from_slice(&FAKE_IP);
                    r.extend_from_slice(&a[8..18]); // 問い合わせた側の MAC と IP
                    return Some(eth(src_mac, 0x0806, &r));
                }
                None
            }
            0x0800 => {
                let ip = &f[14..];
                if ip.len() < 20 || ip[16..20] != FAKE_IP || checksum(&ip[..20]) != 0 {
                    return None;
                }
                let ihl = (ip[0] & 15) as usize * 4;
                let total = be16(ip, 2) as usize;
                let src: [u8; 4] = ip[12..16].try_into().unwrap();
                let body = ip.get(ihl..total)?;
                let (proto, payload) = match ip[9] {
                    1 if body.first() == Some(&8) && checksum(body) == 0 => {
                        // ping: 種類を 0 にしてチェックサムを入れ直す
                        let mut r = body.to_vec();
                        r[0] = 0;
                        r[2] = 0;
                        r[3] = 0;
                        let c = checksum(&r);
                        r[2..4].copy_from_slice(&c.to_be_bytes());
                        (1, r)
                    }
                    17 if body.len() >= 8 => {
                        let (sport, dport) = (be16(body, 0), be16(body, 2));
                        let data = body.get(8..be16(body, 4) as usize)?;
                        let reply = match dport {
                            53 => self.dns_reply(data)?,
                            123 => self.ntp_reply(data)?,
                            _ => return None,
                        };
                        (17, udp(FAKE_IP, src, dport, sport, &reply))
                    }
                    _ => return None,
                };
                Some(eth(src_mac, 0x0800, &ipv4(FAKE_IP, src, proto, &payload)))
            }
            _ => None,
        }
    }

    /// DNS: A の問い合わせに、設定した名前なら答え、無ければ NXDOMAIN
    fn dns_reply(&self, q: &[u8]) -> Option<Vec<u8>> {
        if q.len() < 12 || be16(q, 4) != 1 {
            return None;
        }
        let mut name = String::new();
        let mut i = 12;
        loop {
            let len = *q.get(i)? as usize;
            if len == 0 {
                break;
            }
            if len > 63 {
                return None;
            }
            if !name.is_empty() {
                name.push('.');
            }
            name.push_str(std::str::from_utf8(q.get(i + 1..i + 1 + len)?).ok()?);
            i += 1 + len;
        }
        let qend = i + 5; // 名前の終わりの 0 + 種類 + クラス
        let question = q.get(12..qend)?;
        let addr = self.setup.dns.get(&name.to_ascii_lowercase()).copied();
        let mut r = Vec::new();
        r.extend_from_slice(&q[0..2]);
        r.extend_from_slice(&(if addr.is_some() { 0x8180u16 } else { 0x8183 }).to_be_bytes());
        r.extend_from_slice(&[0, 1, 0, addr.is_some() as u8, 0, 0, 0, 0]);
        r.extend_from_slice(question);
        if let Some(a) = addr {
            r.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0x0E, 0x10, 0, 4]);
            r.extend_from_slice(&a);
        }
        Some(r)
    }

    /// NTP: 決めた時刻を返す
    fn ntp_reply(&self, q: &[u8]) -> Option<Vec<u8>> {
        let t = self.setup.ntp?;
        if q.len() < 48 || q[0] & 7 != 3 {
            return None;
        }
        let ntp = (t + 2_208_988_800) as u32;
        let mut r = vec![0u8; 48];
        r[0] = (4 << 3) | 4;
        r[1] = 2;
        r[24..32].copy_from_slice(&q[40..48]);
        for at in [16, 32, 40] {
            r[at..at + 4].copy_from_slice(&ntp.to_be_bytes());
        }
        Some(r)
    }
}

// --- パケットの組み立てと解析（ゲストとは別の実装） ----------------------------------

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for c in data.chunks(2) {
        sum += u32::from(c[0]) << 8 | u32::from(*c.get(1).unwrap_or(&0));
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn pseudo(src: &[u8], dst: &[u8], proto: u8, seg: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + seg.len());
    v.extend_from_slice(src);
    v.extend_from_slice(dst);
    v.push(0);
    v.push(proto);
    v.extend_from_slice(&(seg.len() as u16).to_be_bytes());
    v.extend_from_slice(seg);
    v
}

fn eth(dst: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&dst);
    f.extend_from_slice(&FAKE_MAC);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f.resize(f.len().max(60), 0);
    f
}

fn ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![0x45, 0];
    p.extend_from_slice(&((20 + payload.len()) as u16).to_be_bytes());
    p.extend_from_slice(&[0x12, 0x34, 0, 0, 64, proto, 0, 0]);
    p.extend_from_slice(&src);
    p.extend_from_slice(&dst);
    let c = checksum(&p);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

fn udp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, data: &[u8]) -> Vec<u8> {
    let mut u = Vec::new();
    u.extend_from_slice(&sport.to_be_bytes());
    u.extend_from_slice(&dport.to_be_bytes());
    u.extend_from_slice(&((8 + data.len()) as u16).to_be_bytes());
    u.extend_from_slice(&[0, 0]);
    u.extend_from_slice(data);
    let c = checksum(&pseudo(&src, &dst, 17, &u));
    u[6..8].copy_from_slice(&(if c == 0 { 0xFFFF } else { c }).to_be_bytes());
    u
}

fn ip_str(b: &[u8]) -> String {
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn hex(b: &[u8]) -> String {
    b.iter().take(64).map(|x| format!("{x:02x}")).collect()
}

/// フレームの要約（種類, 送り元, 宛先）。チェックサムなどが合わなければ Err
pub struct Packet {
    pub kind: String,
    pub src: String,
    pub dst: String,
}

pub fn describe(f: &[u8]) -> std::result::Result<Packet, String> {
    if f.len() < 14 {
        return Err(format!("short frame ({} bytes)", f.len()));
    }
    let pk = |kind: &str, src: String, dst: String| Ok(Packet { kind: kind.into(), src, dst });
    match be16(f, 12) {
        0x0806 => {
            let a = &f[14..];
            if a.len() < 28 {
                return Err("short ARP".into());
            }
            if be16(a, 0) != 1 || be16(a, 2) != 0x0800 || a[4] != 6 || a[5] != 4 {
                return Err("bad ARP header".into());
            }
            let kind = match be16(a, 6) {
                1 => "arp-request",
                2 => "arp-reply",
                _ => return Err("bad ARP op".into()),
            };
            // 返事は Ethernet の宛先が問い合わせた本人、送り元の MAC は ARP の中身と同じはず
            if f[6..12] != a[8..14] {
                return Err("ARP sender MAC differs from Ethernet source".into());
            }
            pk(kind, ip_str(&a[14..18]), ip_str(&a[24..28]))
        }
        0x0800 => {
            let ip = &f[14..];
            if ip.len() < 20 || ip[0] >> 4 != 4 {
                return Err("bad IPv4 header".into());
            }
            let ihl = (ip[0] & 15) as usize * 4;
            let total = be16(ip, 2) as usize;
            if ihl < 20 || total > ip.len() || total < ihl {
                return Err("bad IPv4 length".into());
            }
            if checksum(&ip[..ihl]) != 0 {
                return Err("bad IPv4 header checksum".into());
            }
            let (src, dst) = (&ip[12..16], &ip[16..20]);
            let body = &ip[ihl..total];
            let (s, d) = (ip_str(src), ip_str(dst));
            match ip[9] {
                1 => {
                    if body.len() < 8 || checksum(body) != 0 {
                        return Err("bad ICMP checksum".into());
                    }
                    let kind = match body[0] {
                        0 => "icmp-echo-reply",
                        8 => "icmp-echo-request",
                        3 => "icmp-unreachable",
                        _ => "icmp",
                    };
                    pk(kind, s, d)
                }
                17 => {
                    if body.len() < 8 || (be16(body, 4) as usize) > body.len() {
                        return Err("bad UDP length".into());
                    }
                    if be16(body, 6) != 0 && checksum(&pseudo(src, dst, 17, &body[..be16(body, 4) as usize])) != 0 {
                        return Err("bad UDP checksum".into());
                    }
                    let (sp, dp) = (be16(body, 0), be16(body, 2));
                    let kind = match (sp, dp) {
                        (68, 67) => "dhcp-request".to_string(),
                        (67, 68) => "dhcp-reply".to_string(),
                        _ => format!("udp-{dp}"),
                    };
                    pk(&kind, s, d)
                }
                6 => {
                    if body.len() < 20 || checksum(&pseudo(src, dst, 6, body)) != 0 {
                        return Err("bad TCP checksum".into());
                    }
                    let flags = body[13];
                    let kind = if flags & 0x04 != 0 {
                        "tcp-rst"
                    } else if flags & 0x12 == 0x12 {
                        "tcp-synack"
                    } else if flags & 0x02 != 0 {
                        "tcp-syn"
                    } else if flags & 0x01 != 0 {
                        "tcp-fin"
                    } else {
                        "tcp"
                    };
                    pk(kind, s, d)
                }
                p => pk(&format!("ip-{p}"), s, d),
            }
        }
        t => pk(&format!("eth-{t:04x}"), String::new(), String::new()),
    }
}

/// libpcap の形式で書く（Wireshark・tcpdump で見られる）
fn write_pcap(path: &Path, frames: &[(SystemTime, Vec<u8>)]) -> std::io::Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(&0xa1b2c3d4u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&65535u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // Ethernet
    for (t, f) in frames {
        let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        out.extend_from_slice(&(d.as_secs() as u32).to_le_bytes());
        out.extend_from_slice(&d.subsec_micros().to_le_bytes());
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(f);
    }
    fs::write(path, out)
}

// --- HTTP ------------------------------------------------------------------------

/// guestfwd から接続ごとに起動される: 標準入力から要求を読み、DIR のファイルを標準出力に返す。
/// 標準入出力と標準エラー出力のどれもがゲストへの接続になっている
/// www-raw の返事を置くファイルの名前の後ろ
pub const RAW_SUFFIX: &str = ".http-raw";
/// www-raw の返事の中の、20 秒待つ所の印
pub const RAW_WAIT: &str = "\u{0}WAIT\u{0}";

pub fn http_serve(dir: &Path) -> std::process::ExitCode {
    let mut req = Vec::new();
    let mut buf = [0u8; 1024];
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
        match input.read(&mut buf) {
            Ok(0) | Err(_) => return std::process::ExitCode::FAILURE,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
        if req.len() > 8192 {
            return std::process::ExitCode::FAILURE;
        }
    }
    let text = String::from_utf8_lossy(&req);
    let line = text.lines().next().unwrap_or("");
    // 標準エラー出力も接続につながっている（QEMU の guestfwd）ので、何も書かないこと
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let file = path.trim_start_matches('/');
    if method == "GET" && !file.is_empty() && !file.contains("..") {
        if let Ok(raw) = fs::read_to_string(dir.join(format!("{file}{RAW_SUFFIX}"))) {
            let mut out = std::io::stdout().lock();
            for (i, part) in raw.split(RAW_WAIT).enumerate() {
                if i > 0 {
                    std::thread::sleep(Duration::from_secs(20));
                }
                let _ = out.write_all(part.as_bytes());
                let _ = out.flush();
            }
            return std::process::ExitCode::SUCCESS;
        }
    }
    let ext = file.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    let content_type = match ext.as_str() {
        "htm" | "html" => "text/html",
        "txt" => "text/plain",
        "png" => "image/png",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    };
    let (status, body) = if method != "GET" {
        ("405 Method Not Allowed", b"method not allowed\n".to_vec())
    } else if file.is_empty() || file.contains("..") {
        ("404 Not Found", b"not found\n".to_vec())
    } else {
        match fs::read(dir.join(file)) {
            Ok(b) => ("200 OK", b),
            Err(_) => ("404 Not Found", b"not found\n".to_vec()),
        }
    };
    let mut out = std::io::stdout().lock();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&body);
    let _ = out.flush();
    std::process::ExitCode::SUCCESS
}

/// ホストからゲストの httpd に GET する。(ステータス, 中身)
pub fn http_get(port: u16, path: &str, timeout: Duration) -> Result<(u16, Vec<u8>)> {
    let mut s = connect(port, timeout)?;
    http_exchange(&mut s, path)
}

fn connect(port: u16, timeout: Duration) -> Result<TcpStream> {
    let s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("connect: {e}"))?;
    s.set_read_timeout(Some(timeout)).unwrap();
    s.set_write_timeout(Some(timeout)).unwrap();
    Ok(s)
}

fn http_exchange(s: &mut TcpStream, path: &str) -> Result<(u16, Vec<u8>)> {
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: vibeos\r\n\r\n").as_bytes()).map_err(|e| format!("send: {e}"))?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).map_err(|e| format!("receive ({} bytes so far): {e}", resp.len()))?;
    let end = resp.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no HTTP header in response")? + 4;
    let head = String::from_utf8_lossy(&resp[..end]).into_owned();
    let status: u16 =
        head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or(format!("bad status line: {head}"))?;
    let body = resp[end..].to_vec();
    if let Some(len) = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim()))
    {
        if len.parse::<usize>().ok() != Some(body.len()) {
            return Err(format!("Content-Length {len} but got {} bytes", body.len()));
        }
    }
    Ok((status, body))
}

/// 2 本同時につなぐ: 1 本目は要求を送らずに待たせ、その間に 2 本目で取得し、最後に 1 本目でも取得する
pub fn http_get_concurrent(port: u16, path: &str, timeout: Duration) -> Result<[(u16, Vec<u8>); 2]> {
    let mut first = connect(port, timeout)?;
    thread::sleep(Duration::from_millis(300));
    let second = http_get(port, path, timeout).map_err(|e| format!("second connection: {e}"))?;
    let first = http_exchange(&mut first, path).map_err(|e| format!("first connection: {e}"))?;
    Ok([first, second])
}

/// 決まった中身のファイル（でたらめに見えるが毎回同じ）
pub fn pattern_file(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 24) as u8
        })
        .collect()
}

/// "2026-10-12T01:02:03" か 1970 年からの秒（UTC）
pub fn parse_time(s: &str) -> Option<u64> {
    if let Ok(v) = s.parse() {
        return Some(v);
    }
    let (date, time) = s.split_once('T')?;
    let d: Vec<i64> = date.split('-').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let t: Vec<i64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let ([y, m, day], [hh, mm, ss]) = (d[..].try_into().ok()?, t[..].try_into().ok()?);
    // 1970-01-01 からの日数（Howard Hinnant の方法）
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}
