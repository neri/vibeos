//! tee [-a] ファイル...: 標準入力を標準出力とファイルの両方に書く（-a は書き足す）

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_rt::abi::{self, OPEN_APPEND, OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE};
use vibe_rt::{eprintln, io, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let mut append = false;
    let mut fds = Vec::new();
    let mut code = 0;
    for a in vibe_rt::args().skip(1) {
        if a == "-a" {
            append = true;
            continue;
        }
        let mode = if append { OPEN_APPEND } else { OPEN_TRUNCATE };
        let fd = sys::open_with(a, OPEN_WRITE | OPEN_CREATE | mode);
        if fd < 0 {
            eprintln!("tee: {a}: {}", abi::err::name(-fd));
            code = 1;
        } else {
            fds.push(fd);
        }
    }
    let mut buf = [0u8; 1024];
    loop {
        let n = sys::read(abi::STDIN, &mut buf);
        if n <= 0 {
            break;
        }
        let data = &buf[..n as usize];
        let _ = io::write_all(abi::STDOUT, data);
        for &fd in &fds {
            if io::write_all(fd, data).is_err() {
                code = 1;
            }
        }
    }
    for fd in fds {
        sys::close(fd);
    }
    code
}
