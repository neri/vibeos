//! df: ディスクの空きと、まだフロッピーに書き戻していない変更

#![no_std]
#![no_main]

use vibe_rt::{abi, eprintln, println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let i = match sys::fs_info() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("df: {}", abi::err::name(e));
            return 1;
        }
    };
    println!("{} KB free of {} KB", i.free_bytes / 1024, i.total_bytes / 1024);
    if i.dirty_sectors > 0 {
        println!("{} sectors not saved (use sync)", i.dirty_sectors);
    }
    0
}
