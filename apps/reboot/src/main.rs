//! reboot: 計算機を起動し直す

#![no_std]
#![no_main]

use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    println!("rebooting...");
    sys::reboot()
}
