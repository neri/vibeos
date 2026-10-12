//! get: HTTP/1.0 でファイルを取ってきて保存する
//!
//! get URL [ファイル]   URL は http://ホスト[:ポート]/パス。ファイルを省くとパスの最後の名前
//! （/ で終わるなら INDEX.HTM）。進み具合を表示する。保存した後はフロッピーに書き戻す（sync）

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::abi::{OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE, TICK_HZ};
use vibe_rt::net::{self, Socket, http};
use vibe_rt::{print, println, sys};

vibe_rt::entry!(main);

/// 返事を待つ上限（この間なにも届かなければ諦める）
const TIMEOUT_MS: u32 = 15_000;

fn main() -> i32 {
    let args: Vec<&str> = vibe_rt::args().collect();
    let (url, file) = match args[..] {
        [_, url] => (url, None),
        [_, url, file] => (url, Some(file)),
        _ => {
            println!("usage: get http://host[:port]/path [file]");
            return 2;
        }
    };
    match run(url, file) {
        Ok(()) => 0,
        Err(msg) => {
            println!("get: {msg}");
            1
        }
    }
}

fn run(url: &str, file: Option<&str>) -> Result<(), String> {
    let u = http::parse_url(url).map_err(|e| String::from(e.message()))?;
    let file: String = match file {
        Some(f) => f.into(),
        None => match u.path.rsplit('/').next() {
            Some(name) if !name.is_empty() => name.into(),
            _ => "INDEX.HTM".into(),
        },
    };
    let ip = net::resolve(u.host).map_err(|e| alloc::format!("{}: {}", u.host, e.message()))?;
    println!("connecting to {} ({ip}) port {}", u.host, u.port);
    let sock = Socket::connect(ip, u.port).map_err(|e| alloc::format!("connect: {}", net::err_name(e)))?;
    sock.write_all(&http::build_get(&u)).map_err(|e| alloc::format!("send: {}", net::err_name(e)))?;

    // 見出しを読む
    let mut buf = alloc::vec![0u8; 4096];
    let mut head = Vec::new();
    let (status, length, location, mut body) = loop {
        let n = sock.read(&mut buf, TIMEOUT_MS).map_err(|e| alloc::format!("receive: {}", net::err_name(e)))?;
        if n == 0 {
            return Err("connection closed before the header".into());
        }
        head.extend_from_slice(&buf[..n]);
        if let Some((h, end)) = http::parse_response_head(&head).map_err(|e| String::from(e.message()))? {
            let location = http::header(&head[..end], "location").map(String::from);
            break (h.status, h.content_length, location, head.split_off(end));
        }
    };
    if status != 200 {
        // 移動先は表示だけする（https:// が多く、VibeOS は TLS を扱わない）
        return Err(match location {
            Some(l) => alloc::format!("HTTP {status} {} (Location: {l})", http::status_text(status)),
            None => alloc::format!("HTTP {status} {}", http::status_text(status)),
        });
    }

    let fd = sys::open_with(&file, OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE);
    if fd < 0 {
        return Err(alloc::format!("{file}: {}", net::err_name(-fd)));
    }
    let start = sys::ticks();
    let mut total = 0usize;
    let mut last_shown = 0usize;
    let result = loop {
        if !body.is_empty() {
            let w = sys::write(fd, &body);
            if w < 0 || w as usize != body.len() {
                break Err(alloc::format!("{file}: write failed ({})", net::err_name(-w.min(0))));
            }
            total += body.len();
            body.clear();
            if total - last_shown >= 16 * 1024 {
                show_progress(total, length);
                last_shown = total;
            }
        }
        if length.is_some_and(|l| total >= l) {
            break Ok(());
        }
        match sock.read(&mut buf, TIMEOUT_MS) {
            Ok(0) if length.is_some() => break Err(alloc::format!("connection closed after {total} bytes")),
            Ok(0) => break Ok(()),
            Ok(n) => body.extend_from_slice(&buf[..n]),
            Err(e) => break Err(alloc::format!("receive: {}", net::err_name(e))),
        }
    };
    sys::close(fd);
    show_progress(total, length);
    println!();
    result?;
    let ms = sys::ticks().wrapping_sub(start) * (1000 / TICK_HZ);
    println!("saved {total} bytes to {file} ({} KB/s)", total as u32 / ms.max(1) * 1000 / 1024);
    // フロッピーのドライブがあれば書き戻す（無ければ RAM ディスクにだけ残る）
    let synced = sys::sync();
    if synced < 0 {
        println!("(not saved to the floppy: {})", net::err_name(-synced));
    }
    Ok(())
}

fn show_progress(total: usize, length: Option<usize>) {
    match length {
        Some(l) if l > 0 => print!("\r{total} / {l} bytes ({}%)", total * 100 / l),
        _ => print!("\r{total} bytes"),
    }
}
