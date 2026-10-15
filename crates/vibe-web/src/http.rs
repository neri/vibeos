//! HTTP の要求の組み立てと、返事を届いた順に読む `Reader`
//!
//! - 要求は HTTP/1.0 の GET（`Connection: close`）。`Host` には 80 番でなければポートも付ける
//! - 返事は見出し（ステータス・`Content-Type`・`Content-Length`・`Location`・`Transfer-Encoding`）を読み、
//!   本文は `Content-Length` まで、chunked なら 0 の塊まで、どちらも無ければ接続が閉じるまで
//! - 本文は `limit` バイトまで持ち、越えた分は捨てて `truncated` にする（読み続けはする）

use alloc::string::String;
use alloc::vec::Vec;

use crate::url::Url;

/// 見出しの長さの上限
pub const MAX_HEAD: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpError {
    /// 見出しが読めない
    Malformed,
    /// 見出しが長すぎる
    TooLong,
    /// chunked の塊の大きさが読めない
    BadChunk,
    /// 本文が終わる前に閉じた
    Incomplete,
}

impl HttpError {
    pub fn message(self) -> &'static str {
        match self {
            HttpError::Malformed => "malformed HTTP header",
            HttpError::TooLong => "HTTP header too long",
            HttpError::BadChunk => "malformed chunked data",
            HttpError::Incomplete => "the connection was closed too early",
        }
    }
}

/// GET の要求
pub fn request(url: &Url) -> Vec<u8> {
    let mut s = String::new();
    s.push_str("GET ");
    s.push_str(&url.path);
    s.push_str(" HTTP/1.0\r\nHost: ");
    s.push_str(&url.host);
    if url.port != 80 {
        s.push_str(&alloc::format!(":{}", url.port));
    }
    s.push_str(
        "\r\nUser-Agent: VibeOS-browse/1.0\r\nAccept: text/html, text/plain, image/png, image/bmp, */*;q=0.1\r\n\
         Connection: close\r\n\r\n",
    );
    s.into_bytes()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    /// ステータスの行の理由（"Not Found" など）
    pub reason: String,
    /// `Content-Type` の形式（小文字、`;` からは除く）
    pub mime: Option<String>,
    /// `Content-Type` の charset
    pub charset: Option<String>,
    pub length: Option<usize>,
    pub location: Option<String>,
    pub chunked: bool,
}

impl Head {
    /// 移動の返事か（`Location` も要る）
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308) && self.location.is_some()
    }
}

/// 見出しを読む（`buf` は空行まで）
fn parse_head(buf: &[u8]) -> Result<Head, HttpError> {
    let text = String::from_utf8_lossy(buf);
    let mut lines = text.split('\n').map(|l| l.trim_end_matches('\r'));
    let status_line = lines.next().ok_or(HttpError::Malformed)?;
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next().is_some_and(|v| v.starts_with("HTTP/")) {
        return Err(HttpError::Malformed);
    }
    let status = parts.next().and_then(|s| s.trim().parse().ok()).ok_or(HttpError::Malformed)?;
    let mut head = Head { status, reason: String::from(parts.next().unwrap_or("").trim()), ..Head::default() };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-type") {
            let mime = value.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            head.mime = (!mime.is_empty()).then_some(mime);
            head.charset = crate::charset::label_in_content_type(value).map(String::from);
        } else if name.eq_ignore_ascii_case("content-length") {
            head.length = Some(value.parse().map_err(|_| HttpError::Malformed)?);
        } else if name.eq_ignore_ascii_case("location") {
            head.location = Some(String::from(value));
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            head.chunked = value.split(',').any(|v| v.trim().eq_ignore_ascii_case("chunked"));
        }
    }
    Ok(head)
}

/// chunked の読み方の状態
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chunk {
    /// 塊の大きさの行を読んでいる
    Size,
    /// 塊の中身（残り）
    Data(usize),
    /// 中身の後ろの改行
    DataEnd,
    /// 最後の塊の後ろ（空行まで読み飛ばす）
    Trailer,
    Done,
}

pub struct Reader {
    limit: usize,
    /// 見出しの途中まで・chunked の大きさの行の途中まで
    pending: Vec<u8>,
    head: Option<Head>,
    body: Vec<u8>,
    /// 本文として受け取った量（捨てた分も含む）
    received: usize,
    truncated: bool,
    chunk: Chunk,
}

impl Reader {
    pub fn new(limit: usize) -> Reader {
        Reader {
            limit,
            pending: Vec::new(),
            head: None,
            body: Vec::new(),
            received: 0,
            truncated: false,
            chunk: Chunk::Size,
        }
    }

    pub fn head(&self) -> Option<&Head> {
        self.head.as_ref()
    }

    /// 本文として受け取ったバイト数
    pub fn received(&self) -> usize {
        self.received
    }

    /// 届いたものを渡す
    pub fn feed(&mut self, data: &[u8]) -> Result<(), HttpError> {
        if self.head.is_none() {
            let start = self.pending.len().saturating_sub(3);
            self.pending.extend_from_slice(data);
            let Some(end) = head_end(&self.pending, start) else {
                return if self.pending.len() > MAX_HEAD { Err(HttpError::TooLong) } else { Ok(()) };
            };
            let head = parse_head(&self.pending[..end])?;
            let rest = self.pending.split_off(end);
            self.pending.clear();
            let chunked = head.chunked;
            self.head = Some(head);
            if chunked {
                return self.feed_chunked(&rest);
            }
            self.push_body(&rest);
            return Ok(());
        }
        if self.is_chunked() {
            return self.feed_chunked(data);
        }
        self.push_body(data);
        Ok(())
    }

    /// 本文を足す（`Content-Length` を越えた分は捨てる）
    fn push_body(&mut self, data: &[u8]) {
        let data = match self.head.as_ref().filter(|h| !h.chunked).and_then(|h| h.length) {
            Some(len) => &data[..data.len().min(len.saturating_sub(self.received))],
            None => data,
        };
        self.received += data.len();
        let room = self.limit.saturating_sub(self.body.len());
        if data.len() > room {
            self.truncated = true;
        }
        self.body.extend_from_slice(&data[..data.len().min(room)]);
    }

    fn is_chunked(&self) -> bool {
        self.head.as_ref().is_some_and(|h| h.chunked)
    }

    fn feed_chunked(&mut self, mut data: &[u8]) -> Result<(), HttpError> {
        while !data.is_empty() {
            match self.chunk {
                Chunk::Size | Chunk::Trailer | Chunk::DataEnd => {
                    let Some(nl) = data.iter().position(|&b| b == b'\n') else {
                        self.pending.extend_from_slice(data);
                        if self.pending.len() > 1024 {
                            return Err(HttpError::BadChunk);
                        }
                        return Ok(());
                    };
                    self.pending.extend_from_slice(&data[..nl]);
                    data = &data[nl + 1..];
                    let line = core::mem::take(&mut self.pending);
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim();
                    match self.chunk {
                        Chunk::DataEnd => {
                            if !line.is_empty() {
                                return Err(HttpError::BadChunk);
                            }
                            self.chunk = Chunk::Size;
                        }
                        Chunk::Trailer => {
                            if line.is_empty() {
                                self.chunk = Chunk::Done;
                            }
                        }
                        _ => {
                            // 大きさ（16 進）。; からは拡張なので捨てる
                            let hex = line.split(';').next().unwrap_or("").trim();
                            let n = usize::from_str_radix(hex, 16).map_err(|_| HttpError::BadChunk)?;
                            self.chunk = if n == 0 { Chunk::Trailer } else { Chunk::Data(n) };
                        }
                    }
                }
                Chunk::Data(left) => {
                    let n = left.min(data.len());
                    self.push_body(&data[..n]);
                    data = &data[n..];
                    self.chunk = if n == left { Chunk::DataEnd } else { Chunk::Data(left - n) };
                }
                Chunk::Done => return Ok(()),
            }
        }
        Ok(())
    }

    /// 本文が終わった（`Content-Length` まで届いた・最後の塊を読んだ）。どちらも無ければ閉じるまで false
    pub fn done(&self) -> bool {
        match &self.head {
            None => false,
            Some(h) if h.chunked => self.chunk == Chunk::Done,
            Some(h) => h.length.is_some_and(|len| self.received >= len),
        }
    }

    /// 接続が閉じた後に呼ぶ。（見出し, 本文, 上限で切ったか）
    pub fn finish(self) -> Result<(Head, Vec<u8>, bool), HttpError> {
        let done = self.done();
        let head = self.head.ok_or(HttpError::Incomplete)?;
        let complete = done || (!head.chunked && head.length.is_none());
        if !complete {
            return Err(HttpError::Incomplete);
        }
        Ok((head, self.body, self.truncated))
    }
}

/// 見出しの終わり（空行の後ろ）。`from` から探す
fn head_end(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < buf.len() {
        if buf[i] == b'\n' {
            if buf[i + 1..].starts_with(b"\r\n") {
                return Some(i + 3);
            }
            if buf.get(i + 1) == Some(&b'\n') {
                return Some(i + 2);
            }
        }
        i += 1;
    }
    None
}

/// ステータスの番号の説明（理由が空のとき）
pub fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}
