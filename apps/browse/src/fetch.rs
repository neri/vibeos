//! ページを取ってくる（フロッピーのファイル）
//!
//! 返すものは中身のバイト列と形式（`Content-Type` と同じ形の小文字。`text/html` など）と、最後に
//! 行き着いた URL（フォルダなら `/` を足したもの）。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_rt::abi::{DirEntry, Stat, err};
use vibe_rt::sys;
use vibe_web::url::{Scheme, Url};

use crate::pages;

/// 本文の大きさの上限（これより大きいものは途中まで）
pub const MAX_BODY: usize = 1024 * 1024;

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
        Scheme::Http => {
            let _ = progress;
            Err(String::from("HTTP is not available yet"))
        }
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

/// ファイルの名前（拡張子）から形式を決める。分からなければ、中身に 0 が無ければ文字とみなす
fn mime_of(path: &str, body: &[u8]) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.contains('/')).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "htm" | "html" => "text/html",
        "txt" | "cfg" | "sh" | "md" | "c" | "rs" | "asm" | "inc" | "log" => "text/plain",
        "bmp" => "image/bmp",
        "png" => "image/png",
        "elf" | "sys" | "vfn" | "via" | "img" | "gz" => "application/octet-stream",
        _ if !body[..body.len().min(512)].contains(&0) => "text/plain",
        _ => "application/octet-stream",
    }
}
