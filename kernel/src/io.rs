//! ファイル記述子での読み書き（端末・ファイル・パイプ・ソケットをまとめて扱う）
//!
//! システムコールの read / write と、カーネルの中のシェルの入出力がここを通る。エラーは
//! vibe_abi::err の番号。表の 1 件（`fs::Desc`）を取り出してから（表のロックを手放してから）
//! 読み書きするので、パイプや端末で待っても表のロックは持っていない。

use alloc::string::String;

use vibe_abi::err;

use crate::fs::{self, Desc, Fd};
use crate::net;
use crate::process;
use crate::shell::line::{Input, LineEditor};

/// 読んだバイト数（0 は終わり）。端末からはキーボードから編集付きで 1 行（Ctrl+C で 0）
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, i32> {
    let desc = fs::get(fd).map_err(process::errno_of)?;
    match &*desc {
        Desc::Console => {
            let line = match LineEditor::new().read_line("") {
                Input::Line(mut l) => {
                    l.push('\n');
                    l
                }
                Input::Cancelled => String::new(),
            };
            let n = line.len().min(buf.len());
            buf[..n].copy_from_slice(&line.as_bytes()[..n]);
            Ok(n)
        }
        Desc::File(f) => fs::read_open(&mut f.lock(), buf).map_err(process::errno_of),
        Desc::Socket(sock) => net::recv_from(sock, buf, u32::MAX).map(|r| r.0),
        Desc::PipeRead(p) => p.read(buf),
        Desc::PipeWrite(_) => Err(err::EBADF),
    }
}

/// 書いたバイト数。読む側のいないパイプなら EPIPE
pub fn write(fd: Fd, data: &[u8]) -> Result<usize, i32> {
    let desc = fs::get(fd).map_err(process::errno_of)?;
    match &*desc {
        Desc::Console => {
            kprint!("{}", String::from_utf8_lossy(data));
            Ok(data.len())
        }
        Desc::File(f) => fs::write_open(&mut f.lock(), data).map_err(process::errno_of),
        Desc::Socket(sock) => {
            if sock.kind() != net::Kind::Stream {
                return Err(err::ENOTCONN); // UDP・ICMP は sendto で宛先を付ける
            }
            net::send_all(sock, data)
        }
        Desc::PipeWrite(p) => p.write(data),
        Desc::PipeRead(_) => Err(err::EBADF),
    }
}

/// 全部書く（途中で止まったらエラー）
pub fn write_all(fd: Fd, mut data: &[u8]) -> Result<(), i32> {
    while !data.is_empty() {
        let n = write(fd, data)?;
        if n == 0 {
            return Err(err::EIO);
        }
        data = &data[n..];
    }
    Ok(())
}

/// 待たずに (読める, 書ける) か（poll 用）
pub fn readiness(desc: &Desc) -> (bool, bool) {
    match desc {
        Desc::Console => (false, true),
        Desc::File(_) => (true, true),
        Desc::Socket(sock) => net::readiness(sock),
        Desc::PipeRead(p) => (p.readable(), false),
        Desc::PipeWrite(p) => (false, p.writable()),
    }
}

/// パイプを作って表に入れる。(読む側, 書く側) の fd
pub fn pipe() -> Result<(Fd, Fd), i32> {
    let (r, w) = crate::pipe::new();
    let rfd = fs::install(alloc::sync::Arc::new(Desc::PipeRead(r))).map_err(process::errno_of)?;
    match fs::install(alloc::sync::Arc::new(Desc::PipeWrite(w))) {
        Ok(wfd) => Ok((rfd, wfd)),
        Err(e) => {
            let _ = fs::close(rfd);
            Err(process::errno_of(e))
        }
    }
}
