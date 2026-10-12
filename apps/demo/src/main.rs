//! demo: 3 つのカーネルスレッドが端末の右上に数を描く（5 秒。マルチタスクの見本）

#![no_std]
#![no_main]

use vibe_rt::{eprintln, println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    if sys::demo() < 0 {
        eprintln!("demo is already running");
        return 1;
    }
    println!("demo started");
    0
}
