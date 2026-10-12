//! DNS（RFC 1035）: A レコードの問い合わせと返事の解析
//!
//! 名前の圧縮（ポインタ）を読める。CNAME は読み飛ばし、返事に入っている A レコードを集める。

use alloc::vec::Vec;

use crate::Ipv4;

pub const PORT: u16 = 53;
const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsError {
    /// 名前が長すぎる・使えない文字がある
    BadName,
    Malformed,
    /// 問い合わせと番号が合わない（別の返事）
    WrongId,
    /// その名前は無い（NXDOMAIN）
    NotFound,
    /// サーバがエラーを返した（RCODE）
    Server(u8),
    /// 名前はあるが IPv4 のアドレスが無い
    NoAddress,
}

impl DnsError {
    pub fn message(self) -> &'static str {
        match self {
            DnsError::BadName => "bad name",
            DnsError::Malformed => "malformed reply",
            DnsError::WrongId => "unexpected reply",
            DnsError::NotFound => "name not found",
            DnsError::Server(_) => "server failure",
            DnsError::NoAddress => "no IPv4 address",
        }
    }
}

fn put_name(p: &mut Vec<u8>, name: &str) -> Result<(), DnsError> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > 253 {
        return Err(DnsError::BadName);
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 || !label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return Err(DnsError::BadName);
        }
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    Ok(())
}

/// A レコードの問い合わせを作る（再帰を頼む）
pub fn build_query(id: u16, name: &str) -> Result<Vec<u8>, DnsError> {
    let mut p = Vec::with_capacity(32 + name.len());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    p.extend_from_slice(&1u16.to_be_bytes()); // 質問 1 つ
    p.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    put_name(&mut p, name)?;
    p.extend_from_slice(&TYPE_A.to_be_bytes());
    p.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(p)
}

fn be16(p: &[u8], at: usize) -> Result<u16, DnsError> {
    Ok(u16::from_be_bytes([*p.get(at).ok_or(DnsError::Malformed)?, *p.get(at + 1).ok_or(DnsError::Malformed)?]))
}

/// 名前を読み飛ばして、その後ろの位置を返す（圧縮されていてもよい）
fn skip_name(p: &[u8], mut at: usize) -> Result<usize, DnsError> {
    loop {
        let len = *p.get(at).ok_or(DnsError::Malformed)?;
        match len {
            0 => return Ok(at + 1),
            l if l & 0xC0 == 0xC0 => return Ok(at + 2), // ポインタ（ここで名前が終わる）
            l if l & 0xC0 == 0 => at += 1 + l as usize,
            _ => return Err(DnsError::Malformed),
        }
    }
}

/// 名前を読む（ポインタをたどる。ループしないよう回数に上限を付ける）
pub fn read_name(p: &[u8], mut at: usize, out: &mut alloc::string::String) -> Result<(), DnsError> {
    for _ in 0..64 {
        let len = *p.get(at).ok_or(DnsError::Malformed)?;
        if len == 0 {
            return Ok(());
        }
        if len & 0xC0 == 0xC0 {
            at = (be16(p, at)? & 0x3FFF) as usize;
            continue;
        }
        let label = p.get(at + 1..at + 1 + len as usize).ok_or(DnsError::Malformed)?;
        if !out.is_empty() {
            out.push('.');
        }
        out.extend(label.iter().map(|&c| c as char));
        at += 1 + len as usize;
    }
    Err(DnsError::Malformed)
}

/// 返事から A レコードのアドレスを取り出す
pub fn parse_response(p: &[u8], id: u16) -> Result<Vec<Ipv4>, DnsError> {
    if p.len() < 12 {
        return Err(DnsError::Malformed);
    }
    if be16(p, 0)? != id {
        return Err(DnsError::WrongId);
    }
    let flags = be16(p, 2)?;
    if flags & 0x8000 == 0 {
        return Err(DnsError::Malformed); // 返事ではない
    }
    match (flags & 0x000F) as u8 {
        0 => {}
        3 => return Err(DnsError::NotFound),
        rcode => return Err(DnsError::Server(rcode)),
    }
    let questions = be16(p, 4)?;
    let answers = be16(p, 6)?;
    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(p, at)? + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        at = skip_name(p, at)?;
        let kind = be16(p, at)?;
        let class = be16(p, at + 2)?;
        let len = be16(p, at + 8)? as usize;
        let data = p.get(at + 10..at + 10 + len).ok_or(DnsError::Malformed)?;
        if kind == TYPE_A && class == CLASS_IN && len == 4 {
            out.push(Ipv4([data[0], data[1], data[2], data[3]]));
        }
        at += 10 + len;
    }
    if out.is_empty() { Err(DnsError::NoAddress) } else { Ok(out) }
}

/// 問い合わせの名前と番号を読む（テスト用のサーバが使う）
pub fn parse_query(p: &[u8]) -> Result<(u16, alloc::string::String), DnsError> {
    if p.len() < 12 || be16(p, 4)? == 0 {
        return Err(DnsError::Malformed);
    }
    let mut name = alloc::string::String::new();
    read_name(p, 12, &mut name)?;
    Ok((be16(p, 0)?, name))
}

/// 問い合わせへの返事を作る（テスト用）。addr が None なら NXDOMAIN
pub fn build_response(query: &[u8], addr: Option<Ipv4>) -> Result<Vec<u8>, DnsError> {
    let end = skip_name(query, 12)? + 4;
    let question = query.get(12..end).ok_or(DnsError::Malformed)?;
    let mut p = Vec::new();
    p.extend_from_slice(&query[0..2]);
    let rcode = if addr.is_some() { 0 } else { 3 };
    p.extend_from_slice(&(0x8180u16 | rcode).to_be_bytes()); // QR, RD, RA
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&(addr.is_some() as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    p.extend_from_slice(question);
    if let Some(a) = addr {
        p.extend_from_slice(&0xC00Cu16.to_be_bytes()); // 質問の名前へのポインタ
        p.extend_from_slice(&TYPE_A.to_be_bytes());
        p.extend_from_slice(&CLASS_IN.to_be_bytes());
        p.extend_from_slice(&300u32.to_be_bytes());
        p.extend_from_slice(&4u16.to_be_bytes());
        p.extend_from_slice(&a.0);
    }
    Ok(p)
}
