//! mem: 物理ページとカーネルのヒープの使い方

#![no_std]
#![no_main]

use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let i = sys::sys_info();
    println!("pages: {} KB free / {} KB", i.free_kb, i.total_kb);
    println!("heap: {} / {} bytes free, {} blocks", i.heap_free, i.heap_total, i.heap_blocks);
    0
}
