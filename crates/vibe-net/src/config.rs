//! BOOT.CFG のネットワークの設定
//!
//! ```text
//! net=auto              カード: PCI（RTL8139・NE2000 PCI）を探し、無ければ ISA の NE2000（0x300, IRQ 10）。既定
//! net=ne2000,0x300,10   ISA の NE2000（I/O ポート, IRQ）。net=none で使わない
//! ip=dhcp               DHCP でもらう（既定）。固定なら ip=10.0.2.15/24
//! gw=10.0.2.2           ゲートウェイ（固定アドレスのとき。DHCP の値より優先）
//! dns=10.0.2.3          DNS サーバ（DHCP の値より優先）
//! ntp=10.0.2.2          NTP サーバ（ntp コマンドの既定）
//! tz=+09:00             時差（NTP の UTC を PC の時計の現地時刻に直す）
//! ```
//! 1 行に 1 つ。`#` から行末まではコメント。知らない行は無視する（video= など）。

use crate::Ipv4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    None,
    /// PCI のカード、無ければ ISA の NE2000（0x300, IRQ 10）
    Auto,
    Ne2000 { io: u16, irq: u8 },
}

pub const DEFAULT_NE2000: Device = Device::Ne2000 { io: 0x300, irq: 10 };

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetSettings {
    pub device: Device,
    /// 固定アドレス（None なら DHCP）
    pub static_ip: Option<(Ipv4, u8)>,
    pub gateway: Option<Ipv4>,
    pub dns: Option<Ipv4>,
    pub ntp: Option<Ipv4>,
    /// UTC との差（分）
    pub tz_minutes: i32,
}

impl Default for NetSettings {
    fn default() -> Self {
        NetSettings { device: Device::Auto, static_ip: None, gateway: None, dns: None, ntp: None, tz_minutes: 0 }
    }
}

/// 読めなかった行（表示用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadLine<'a>(pub &'a str);

fn parse_num(s: &str) -> Option<u32> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// "+09:00", "+9", "-5:30", "0"
fn parse_tz(s: &str) -> Option<i32> {
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => (1, s),
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?),
        None => (rest.parse::<i32>().ok()?, 0),
    };
    (h <= 14 && (0..60).contains(&m)).then_some(sign * (h * 60 + m))
}

fn parse_device(v: &str) -> Option<Device> {
    let mut parts = v.split(',').map(str::trim);
    match parts.next()? {
        "none" | "off" => Some(Device::None),
        "auto" => Some(Device::Auto),
        "ne2000" | "ne2k" => {
            let io = match parts.next() {
                Some(p) => parse_num(p).filter(|&v| v <= 0xFFE0)? as u16,
                None => 0x300,
            };
            let irq = match parts.next() {
                Some(p) => parse_num(p).filter(|&v| (2..16).contains(&v))? as u8,
                None => 10,
            };
            Some(Device::Ne2000 { io, irq })
        }
        _ => None,
    }
}

/// BOOT.CFG の中身を読む。読めなかった行は `bad` に渡す
pub fn parse<'a>(text: &'a str, mut bad: impl FnMut(BadLine<'a>)) -> NetSettings {
    let mut s = NetSettings::default();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let ok = match key {
            "net" => parse_device(value).map(|d| s.device = d).is_some(),
            "ip" if value == "dhcp" => {
                s.static_ip = None;
                true
            }
            "ip" => {
                let (addr, prefix) = value.split_once('/').unwrap_or((value, "24"));
                match (Ipv4::parse(addr), prefix.parse::<u8>()) {
                    (Some(a), Ok(p)) if p <= 32 => {
                        s.static_ip = Some((a, p));
                        true
                    }
                    _ => false,
                }
            }
            "gw" => Ipv4::parse(value).map(|a| s.gateway = Some(a)).is_some(),
            "dns" => Ipv4::parse(value).map(|a| s.dns = Some(a)).is_some(),
            "ntp" => Ipv4::parse(value).map(|a| s.ntp = Some(a)).is_some(),
            "tz" => parse_tz(value).map(|m| s.tz_minutes = m).is_some(),
            _ => true, // ほかの設定（video= など）
        };
        if !ok {
            bad(BadLine(raw));
        }
    }
    s
}
