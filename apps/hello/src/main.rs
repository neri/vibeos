//! 最初のユーザアプリ: あいさつと引数の表示、ヒープの確認

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    println!("Hello from user space! (pid {})", sys::getpid());
    for (i, a) in vibe_rt::args().enumerate() {
        println!("  argv[{i}] = {a}");
    }
    // ヒープ（sbrk）が使えること: 素数を数える
    let primes: Vec<u32> = (2..20000u32).filter(|&n| (2..n).take_while(|d| d * d <= n).all(|d| n % d != 0)).collect();
    println!("primes below 20000: {}", primes.len());
    0
}
