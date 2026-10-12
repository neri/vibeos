//! httpd: フロッピーのファイルを HTTP/1.0 で返す
//!
//! httpd [ポート] [ディレクトリ]   既定は 80 番と /。ディレクトリへの要求には一覧の HTML を返す。
//! 同時に MAX_CONNS 本までの接続を poll で受け付け、要求が全部届いたものから返事を送る。
//! Ctrl+C で止まる。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::abi::{DirEntry, POLL_IN, PollFd, Stat, err};
use vibe_rt::net::{self, Ipv4, Socket, http};
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

const MAX_CONNS: usize = 4;
/// 要求が届くのを待つ上限（刻み）。超えた接続は切る
const REQUEST_TIMEOUT_TICKS: u32 = 30 * vibe_rt::abi::TICK_HZ;

struct Conn {
    sock: Socket,
    peer: Ipv4,
    req: Vec<u8>,
    since: u32,
}

fn main() -> i32 {
    let mut port = 80u16;
    let mut root = String::from("/");
    for a in vibe_rt::args().skip(1) {
        match a.parse::<u16>() {
            Ok(p) if p != 0 => port = p,
            _ if a.starts_with('/') => root = a.trim_end_matches('/').into(),
            _ => {
                println!("usage: httpd [port] [dir]");
                return 2;
            }
        }
    }
    let listener = match Socket::listen(port, MAX_CONNS as u32) {
        Ok(s) => s,
        Err(e) => {
            println!("httpd: port {port}: {}", net::err_name(e));
            return 1;
        }
    };
    match net::net_info() {
        Ok(i) => println!("httpd: serving {} on http://{}:{port}/ (Ctrl+C to stop)", if root.is_empty() { "/" } else { &root }, Ipv4(i.addr)),
        Err(_) => println!("httpd: serving on port {port}"),
    }
    net::catch_ctrl_c();
    let mut conns: Vec<Conn> = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        if net::ctrl_c_pressed() {
            break;
        }
        let mut fds = Vec::with_capacity(conns.len() + 1);
        fds.push(PollFd { fd: listener.0, events: if conns.len() < MAX_CONNS { POLL_IN } else { 0 }, revents: 0 });
        fds.extend(conns.iter().map(|c| PollFd { fd: c.sock.0, events: POLL_IN, revents: 0 }));
        let n = net::poll(&mut fds, 1000);
        if n == -err::EINTR {
            break;
        }
        if n < 0 {
            println!("httpd: poll: {}", net::err_name(-n));
            return 1;
        }
        // 新しい接続
        if fds[0].revents & POLL_IN != 0 {
            match net::accept(listener.0) {
                Ok((fd, peer, _)) => conns.push(Conn { sock: Socket(fd), peer, req: Vec::new(), since: sys::ticks() }),
                Err(e) if e == err::EINTR => break,
                Err(e) => println!("httpd: accept: {}", net::err_name(e)),
            }
        }
        // 要求を読む。全部届いたら返事を送って閉じる
        let mut i = 0;
        while i < conns.len() {
            let ready = fds.get(i + 1).is_some_and(|f| f.revents & POLL_IN != 0 && f.fd == conns[i].sock.0);
            let mut done = false;
            if ready {
                match conns[i].sock.read(&mut buf, 0) {
                    Ok(0) | Err(_) => done = true, // 相手が閉じた・エラー
                    Ok(n) => {
                        conns[i].req.extend_from_slice(&buf[..n]);
                        match http::parse_request_head(&conns[i].req) {
                            Ok(Some((req, _))) => {
                                respond(&conns[i], &req, &root);
                                done = true;
                            }
                            Ok(None) => {}
                            Err(_) => {
                                send_error(&conns[i], "?", "?", 400);
                                done = true;
                            }
                        }
                    }
                }
            } else if sys::ticks().wrapping_sub(conns[i].since) > REQUEST_TIMEOUT_TICKS {
                done = true;
            }
            if done {
                conns.swap_remove(i); // 捨てると閉じる
            } else {
                i += 1;
            }
        }
    }
    println!("httpd: stopped");
    0
}

fn respond(c: &Conn, req: &http::Request, root: &str) {
    if req.method != "GET" && req.method != "HEAD" {
        return send_error(c, &req.method, &req.path, 405);
    }
    if req.path.split('/').any(|p| p == "..") {
        return send_error(c, &req.method, &req.path, 403);
    }
    let path = alloc::format!("{root}{}", req.path);
    let mut st = Stat::default();
    if sys::stat(&path, &mut st) < 0 {
        return send_error(c, &req.method, &req.path, 404);
    }
    let (body, ctype) = if st.is_dir != 0 {
        (listing(&path, &req.path), "text/html")
    } else {
        match vibe_rt::read_file(&path) {
            Ok(b) => (b, http::content_type(&path)),
            Err(_) => return send_error(c, &req.method, &req.path, 500),
        }
    };
    send(c, &req.method, &req.path, 200, ctype, &body);
}

fn send_error(c: &Conn, method: &str, path: &str, status: u16) {
    let body = alloc::format!("{status} {}\n", http::status_text(status));
    send(c, method, path, status, "text/plain", body.as_bytes());
}

fn send(c: &Conn, method: &str, path: &str, status: u16, ctype: &str, body: &[u8]) {
    let mut out = http::build_response_head(status, ctype, body.len());
    if method != "HEAD" {
        out.extend_from_slice(body);
    }
    let r = c.sock.write_all(&out);
    println!("httpd: {} {method} {path} {status} {}{}", c.peer, body.len(), if r.is_err() { " (send failed)" } else { "" });
}

/// ディレクトリの一覧の HTML
fn listing(dir: &str, url_path: &str) -> Vec<u8> {
    let base = url_path.trim_end_matches('/');
    let mut entries = alloc::vec![DirEntry::EMPTY; 128];
    let n = sys::readdir(dir, &mut entries).max(0) as usize;
    let mut title = String::new();
    http::html_escape(if base.is_empty() { "/" } else { url_path }, &mut title);
    let mut h = alloc::format!("<html><head><title>Index of {title}</title></head><body>\n<h1>Index of {title}</h1>\n<ul>\n");
    if !base.is_empty() {
        let parent = base.rsplit_once('/').map_or("", |(p, _)| p);
        h.push_str(&alloc::format!("<li><a href=\"{parent}/\">../</a></li>\n"));
    }
    for e in &entries[..n] {
        let name = e.name();
        if name == "." || name == ".." {
            continue;
        }
        let mut esc = String::new();
        http::html_escape(name, &mut esc);
        if e.is_dir != 0 {
            h.push_str(&alloc::format!("<li><a href=\"{base}/{esc}/\">{esc}/</a></li>\n"));
        } else {
            h.push_str(&alloc::format!("<li><a href=\"{base}/{esc}\">{esc}</a> {} bytes</li>\n", e.size));
        }
    }
    h.push_str("</ul>\n<p>VibeOS httpd</p></body></html>\n");
    h.into_bytes()
}
