//! ページを取ってくる（フロッピーのファイルと HTTP）
//!
//! 返すものは中身のバイト列と形式（`Content-Type` と同じ形の小文字。`text/html` など）と、最後に
//! 行き着いた URL（フォルダなら `/` を足したもの、HTTP なら移動した先）。
//!
//! HTTP: 名前を引き、つなぎ、GET を送って、返事を `vibe_web::http::Reader` で届いた順に読む。読む合間
//! （200ms ごと）に `progress` を呼ぶので、Esc で止められる。移動（301・302・303・307・308）は 5 回まで
//! たどる（先が https なら止める）。200 以外はエラーのページにする。15 秒なにも届かなければ諦める。

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_rt::abi::{DirEntry, Stat, err};
use vibe_rt::net::{self, Socket};
use vibe_rt::{println, sys};
use vibe_web::http::{self, Reader};
use vibe_web::url::{Scheme, Url, UrlError};

use crate::pages;

/// 本文の大きさの上限（これより大きいものは途中まで）
pub const MAX_BODY: usize = 1024 * 1024;
/// 移動をたどる回数の上限
const MAX_REDIRECTS: usize = 5;
/// 読む合間（進み具合を出し、Esc を見る）
const POLL_MS: u32 = 200;
/// この間なにも届かなければ諦める
const IDLE_MS: u32 = 15_000;
/// 止めたときのエラー
pub const STOPPED: &str = "Stopped.";

pub struct Response {
    pub url: Url,
    /// `text/html` など（小文字、`;` からは除く）
    pub mime: String,
    /// `Content-Type` の charset（無ければ None）
    pub charset: Option<String>,
    pub body: Vec<u8>,
    /// 上限で切った
    pub truncated: bool,
}

/// `url` を取ってくる。`progress` は進み具合の文字を受け取り、false を返したら止める（Esc）
pub fn fetch(url: &Url, progress: &mut dyn FnMut(&str) -> bool) -> Result<Response, String> {
    match url.scheme {
        Scheme::File => file(url),
        Scheme::Http => http_get(url, progress),
    }
}

fn file(url: &Url) -> Result<Response, String> {
    let mut path = url.file_path();
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
    let mut st = Stat::default();
    let r = sys::stat(&path, &mut st);
    if r < 0 {
        return Err(String::from(err::name(-r)));
    }
    if st.is_dir != 0 {
        let mut entries = vec![DirEntry::EMPTY; 256];
        let n = sys::readdir(&path, &mut entries);
        if n < 0 {
            return Err(String::from(err::name(-n)));
        }
        entries.truncate(n as usize);
        entries.retain(|e| e.name() != "." && e.name() != "..");
        let mut final_url = url.without_fragment();
        if !final_url.path.ends_with('/') {
            final_url.path.push('/');
        }
        let html = pages::directory(&path, &mut entries);
        return Ok(Response {
            url: final_url,
            mime: String::from("text/html"),
            charset: Some(String::from("utf-8")),
            body: html.into_bytes(),
            truncated: false,
        });
    }
    let mut body = vibe_rt::read_file(&path).map_err(|e| String::from(err::name(e)))?;
    let truncated = body.len() > MAX_BODY;
    body.truncate(MAX_BODY);
    Ok(Response { url: url.clone(), mime: String::from(mime_of(&path, &body)), charset: None, body, truncated })
}

/// ファイルの名前（拡張子）から形式を決める。分からなければ中身を見る（0 があればバイナリ、`<` で始まれば
/// HTML、ほかは文字）
fn mime_of(path: &str, body: &[u8]) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.contains('/')).unwrap_or("");
    let head = &body[..body.len().min(512)];
    match ext.to_ascii_lowercase().as_str() {
        "htm" | "html" => "text/html",
        "txt" | "cfg" | "sh" | "md" | "c" | "rs" | "asm" | "inc" | "log" => "text/plain",
        "bmp" => "image/bmp",
        "png" => "image/png",
        "elf" | "sys" | "vfn" | "via" | "img" | "gz" => "application/octet-stream",
        _ if head.contains(&0) => "application/octet-stream",
        _ if head.trim_ascii_start().starts_with(b"<") => "text/html",
        _ => "text/plain",
    }
}

fn http_get(start: &Url, progress: &mut dyn FnMut(&str) -> bool) -> Result<Response, String> {
    let mut url = start.without_fragment();
    for _ in 0..=MAX_REDIRECTS {
        let (head, body, truncated) = http_once(&url, progress)?;
        if head.is_redirect() {
            let loc = head.location.unwrap_or_default();
            url = match url.join(&loc) {
                Ok(u) => u.without_fragment(),
                Err(UrlError::Https) => return Err(format!("The page moved to {loc}, but HTTPS is not supported.")),
                Err(e) => return Err(format!("The page moved to {loc}: {}", e.message())),
            };
            println!("browse: redirect {url}");
            continue;
        }
        if head.status != 200 {
            let reason = if head.reason.is_empty() { http::status_text(head.status) } else { head.reason.as_str() };
            return Err(format!("HTTP {} {reason}", head.status));
        }
        let mime = match head.mime {
            Some(m) => m,
            None => String::from(mime_of(&url.path, &body)),
        };
        url.fragment = start.fragment.clone();
        return Ok(Response { url, mime, charset: head.charset, body, truncated });
    }
    Err(String::from("Too many redirects."))
}

/// 1 回の GET
fn http_once(url: &Url, progress: &mut dyn FnMut(&str) -> bool) -> Result<(http::Head, Vec<u8>, bool), String> {
    let host = url.host.as_str();
    if !progress(&format!("Looking up {host} ...")) {
        return Err(String::from(STOPPED));
    }
    let ip = net::resolve(host).map_err(|e| format!("Cannot find the server {host}: {}", e.message()))?;
    if !progress(&format!("Connecting to {host} ({ip}) ...")) {
        return Err(String::from(STOPPED));
    }
    let sock = Socket::connect(ip, url.port)
        .map_err(|e| format!("Cannot connect to {host} port {}: {}", url.port, net::err_name(e)))?;
    sock.write_all(&http::request(url)).map_err(|e| format!("Cannot send the request: {}", net::err_name(e)))?;
    let mut reader = Reader::new(MAX_BODY);
    let mut buf = vec![0u8; 4096];
    let mut idle = 0;
    loop {
        match sock.read(&mut buf, POLL_MS) {
            Ok(0) => break,
            Ok(n) => {
                idle = 0;
                reader.feed(&buf[..n]).map_err(|e| String::from(e.message()))?;
                if reader.done() {
                    break;
                }
            }
            Err(err::ETIMEDOUT) => {
                idle += POLL_MS;
                if idle >= IDLE_MS {
                    return Err(format!("{host} did not answer for {} seconds.", IDLE_MS / 1000));
                }
            }
            Err(e) => return Err(format!("Cannot receive: {}", net::err_name(e))),
        }
        let msg = match reader.head() {
            None => format!("Waiting for {host} ..."),
            Some(_) => format!("Received {} bytes from {host} ...", reader.received()),
        };
        if !progress(&msg) {
            return Err(String::from(STOPPED));
        }
    }
    reader.finish().map_err(|e| String::from(e.message()))
}
