//! HTTP/1.0 の最小限（`get` と `httpd` が使う）
//!
//! - クライアント: URL の解析、GET の要求、返事の見出し（ステータスと Content-Length）の解析
//! - サーバ: 要求の 1 行目の解析、見出しの組み立て、拡張子からの Content-Type
//!
//! 接続は 1 回の要求ごとに閉じる（Keep-Alive・chunked は使わない）。

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    /// http:// 以外・形がおかしい URL
    BadUrl,
    /// 見出しが読めない
    Malformed,
    /// 見出しが長すぎる
    TooLong,
}

impl HttpError {
    pub fn message(self) -> &'static str {
        match self {
            HttpError::BadUrl => "bad URL (only http://host[:port]/path)",
            HttpError::Malformed => "malformed HTTP header",
            HttpError::TooLong => "HTTP header too long",
        }
    }
}

/// 見出しの長さの上限
pub const MAX_HEAD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Url<'a> {
    pub host: &'a str,
    pub port: u16,
    /// "/" から始まる
    pub path: &'a str,
}

/// "http://host[:port][/path]" を読む
pub fn parse_url(url: &str) -> Result<Url<'_>, HttpError> {
    let rest = url.strip_prefix("http://").ok_or(HttpError::BadUrl)?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| HttpError::BadUrl)?),
        None => (authority, 80),
    };
    if host.is_empty() || port == 0 || path.bytes().any(|c| c <= b' ') {
        return Err(HttpError::BadUrl);
    }
    Ok(Url { host, port, path })
}

/// GET の要求
pub fn build_get(url: &Url) -> Vec<u8> {
    let mut s = String::new();
    s.push_str("GET ");
    s.push_str(url.path);
    s.push_str(" HTTP/1.0\r\nHost: ");
    s.push_str(url.host);
    if url.port != 80 {
        s.push(':');
        push_num(&mut s, url.port as usize);
    }
    s.push_str("\r\nUser-Agent: VibeOS-get/1.0\r\nConnection: close\r\n\r\n");
    s.into_bytes()
}

fn push_num(s: &mut String, mut n: usize) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    s.extend(digits[i..].iter().map(|&c| c as char));
}

/// 見出しの終わり（空行の後ろ）の位置。まだ届いていなければ None
pub fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).or_else(|| {
        // 改行が LF だけのサーバもある
        buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub content_length: Option<usize>,
}

/// 返事の見出しを読む。見出しがまだ全部届いていなければ Ok(None)
pub fn parse_response_head(buf: &[u8]) -> Result<Option<(ResponseHead, usize)>, HttpError> {
    let Some(end) = head_end(buf) else {
        return if buf.len() > MAX_HEAD { Err(HttpError::TooLong) } else { Ok(None) };
    };
    let text = core::str::from_utf8(&buf[..end]).map_err(|_| HttpError::Malformed)?;
    let mut lines = text.lines();
    let status_line = lines.next().ok_or(HttpError::Malformed)?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().ok_or(HttpError::Malformed)?;
    if !version.starts_with("HTTP/") {
        return Err(HttpError::Malformed);
    }
    let status = parts.next().and_then(|s| s.parse().ok()).ok_or(HttpError::Malformed)?;
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse().map_err(|_| HttpError::Malformed)?);
            }
        }
    }
    Ok(Some((ResponseHead { status, content_length }, end)))
}

/// 見出し（`head_end` までの部分）から名前の値を取り出す（名前の大文字小文字は区別しない）
pub fn header<'a>(head: &'a [u8], name: &str) -> Option<&'a str> {
    let text = core::str::from_utf8(head).ok()?;
    text.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// %XX を戻したパス
    pub path: String,
}

/// 要求の見出しを読む。まだ全部届いていなければ Ok(None)
pub fn parse_request_head(buf: &[u8]) -> Result<Option<(Request, usize)>, HttpError> {
    let Some(end) = head_end(buf) else {
        return if buf.len() > MAX_HEAD { Err(HttpError::TooLong) } else { Ok(None) };
    };
    let text = core::str::from_utf8(&buf[..end]).map_err(|_| HttpError::Malformed)?;
    let line = text.lines().next().ok_or(HttpError::Malformed)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(HttpError::Malformed)?;
    let target = parts.next().ok_or(HttpError::Malformed)?;
    if !target.starts_with('/') {
        return Err(HttpError::Malformed);
    }
    let target = target.split(['?', '#']).next().unwrap_or("/");
    let path = percent_decode(target).ok_or(HttpError::Malformed)?;
    Ok(Some((Request { method: String::from(method), path }, end)))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = core::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

/// 拡張子から Content-Type
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "txt" | "cfg" | "md" => "text/plain",
        "htm" | "html" => "text/html",
        "bmp" => "image/bmp",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

/// 返事の見出し
pub fn build_response_head(status: u16, content_type: &str, length: usize) -> Vec<u8> {
    let mut s = String::from("HTTP/1.0 ");
    push_num(&mut s, status as usize);
    s.push(' ');
    s.push_str(status_text(status));
    s.push_str("\r\nServer: VibeOS-httpd/1.0\r\nContent-Type: ");
    s.push_str(content_type);
    s.push_str("\r\nContent-Length: ");
    push_num(&mut s, length);
    s.push_str("\r\nConnection: close\r\n\r\n");
    s.into_bytes()
}

/// HTML に入れる文字列（& < > " を置き換える）
pub fn html_escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}
