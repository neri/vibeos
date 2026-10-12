//! SNTP（RFC 4330）: 時刻の問い合わせと返事の解析
//!
//! NTP の時刻は 1900 年からの秒（UTC）。ここでは 1970 年からの秒（UTC）に直して返す。

pub const PORT: u16 = 123;
pub const PACKET_LEN: usize = 48;
/// 1900-01-01 から 1970-01-01 までの秒
pub const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SntpError {
    Malformed,
    /// サーバが「使うな」と言ってきた（stratum 0, Kiss-o'-Death）か、時刻を持っていない
    Unsynchronized,
    /// 送った問い合わせへの返事ではない
    WrongReply,
}

/// 問い合わせ（クライアント, 版 4）。`tag` は送信時刻の欄に入れ、返事の originate で照合する
pub fn build_request(tag: u64) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = (4 << 3) | 3; // LI = 0, VN = 4, Mode = 3（クライアント）
    p[40..48].copy_from_slice(&tag.to_be_bytes());
    p
}

/// 返事から送信時刻（1970 年からの秒, UTC）を取り出す
pub fn parse_response(p: &[u8], tag: u64) -> Result<u64, SntpError> {
    if p.len() < PACKET_LEN {
        return Err(SntpError::Malformed);
    }
    let mode = p[0] & 7;
    if mode != 4 && mode != 5 {
        return Err(SntpError::Malformed);
    }
    if u64::from_be_bytes(p[24..32].try_into().unwrap()) != tag {
        return Err(SntpError::WrongReply);
    }
    let li = p[0] >> 6;
    let stratum = p[1];
    if stratum == 0 || li == 3 {
        return Err(SntpError::Unsynchronized);
    }
    let secs = u32::from_be_bytes(p[40..44].try_into().unwrap()) as u64;
    let frac = u32::from_be_bytes(p[44..48].try_into().unwrap());
    // 2036 年に 32bit が一周する（era 1）。1968 年より前に見えるときは次の周とみなす
    let secs = if secs < NTP_UNIX_OFFSET - 2 * 365 * 86400 { secs + (1 << 32) } else { secs };
    if secs < NTP_UNIX_OFFSET {
        return Err(SntpError::Malformed);
    }
    // 端数は四捨五入
    Ok(secs - NTP_UNIX_OFFSET + (frac >= 0x8000_0000) as u64)
}

/// サーバの返事を作る（テスト用）。unix_secs は 1970 年からの秒（UTC）
pub fn build_response(request: &[u8], unix_secs: u64) -> Option<[u8; PACKET_LEN]> {
    if request.len() < PACKET_LEN || request[0] & 7 != 3 {
        return None;
    }
    let ntp = (unix_secs + NTP_UNIX_OFFSET) as u32;
    let mut p = [0u8; PACKET_LEN];
    p[0] = (4 << 3) | 4; // VN = 4, Mode = 4（サーバ）
    p[1] = 2; // stratum
    p[24..32].copy_from_slice(&request[40..48]); // originate = 問い合わせの送信時刻
    for at in [16, 32, 40] {
        p[at..at + 4].copy_from_slice(&ntp.to_be_bytes());
    }
    Some(p)
}
