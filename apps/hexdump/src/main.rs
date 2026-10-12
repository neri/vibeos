//! hexdump ファイル: 16 進で表示する（40 桁の画面に合わせて 1 行 8 バイト）

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;

use vibe_rt::{abi, eprintln, println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let Some(path) = vibe_rt::args().nth(1) else {
        eprintln!("usage: hexdump file");
        return 2;
    };
    let fd = sys::open(path);
    if fd < 0 {
        eprintln!("hexdump: {path}: {}", abi::err::name(-fd));
        return 1;
    }
    let mut buf = [0u8; 8];
    let mut offset = 0u32;
    loop {
        let n = sys::read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        let bytes = &buf[..n as usize];
        let hex: String = bytes.iter().map(|b| alloc::format!("{b:02x} ")).collect();
        let text: String = bytes.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
        println!("{offset:05x} {hex:24}{text}");
        offset += n as u32;
    }
    sys::close(fd);
    0
}
