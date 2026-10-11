//! テスト用: アプリからファイルを作る・書く・読む・消す

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_rt::abi::{self, DirEntry, Stat, err};
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn check(what: &str, got: i32, want: i32) -> bool {
    if got != want {
        println!("fstest: {what}: got {got}, want {want}");
    }
    got == want
}

fn main() -> i32 {
    let mut ok = true;
    ok &= check("mkdir", sys::mkdir("/TMP"), 0);
    ok &= check("mkdir again", sys::mkdir("/TMP"), -err::EEXIST);
    let fd = sys::open_with("/TMP/DATA.BIN", abi::OPEN_WRITE | abi::OPEN_CREATE);
    ok &= fd >= 3;
    let data: Vec<u8> = (0..3000u32).map(|i| (i * 7) as u8).collect();
    ok &= check("write", sys::write(fd, &data), 3000);
    sys::close(fd);
    let mut st = Stat::default();
    ok &= check("stat", sys::stat("/TMP/DATA.BIN", &mut st), 0) && check("size", st.size as i32, 3000);
    ok &= vibe_rt::read_file("/TMP/DATA.BIN").map(|d| d == data).unwrap_or(false);
    // 書き足し
    let fd = sys::open_with("/TMP/DATA.BIN", abi::OPEN_WRITE | abi::OPEN_APPEND);
    ok &= check("append", sys::write(fd, b"END"), 3);
    sys::close(fd);
    ok &= check("size after append", vibe_rt::read_file("/TMP/DATA.BIN").map(|d| d.len() as i32).unwrap_or(-1), 3003);
    // 読み込み専用で開いたものには書けない
    let fd = sys::open("/TMP/DATA.BIN");
    ok &= check("write read-only", sys::write(fd, b"x"), -err::EBADF);
    sys::close(fd);
    let mut entries = [DirEntry::EMPTY; 8];
    ok &= check("readdir", sys::readdir("/TMP", &mut entries), 1) && entries[0].name() == "DATA.BIN";
    ok &= check("rm dir not empty", sys::remove("/TMP"), -err::ENOTEMPTY);
    ok &= check("rm file", sys::remove("/TMP/DATA.BIN"), 0);
    ok &= check("rm dir", sys::remove("/TMP"), 0);
    ok &= check("open missing", sys::open("/TMP/DATA.BIN"), -err::ENOENT);
    ok &= check("bad name", sys::open_with("/TOOLONGNAME.TXT", abi::OPEN_WRITE | abi::OPEN_CREATE), -err::EINVAL);
    println!("fstest: {}", if ok { "all ok" } else { "FAILED" });
    if ok { 0 } else { 1 }
}
