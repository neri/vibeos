//! uptime: 起動からの時間

#![no_std]
#![no_main]

use vibe_rt::abi::TICK_HZ;
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let t = sys::ticks();
    println!("up {}.{:02} s ({} ticks)", t / TICK_HZ, t % TICK_HZ, t);
    0
}
