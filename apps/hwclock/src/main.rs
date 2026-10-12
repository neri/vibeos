//! hwclock: CMOS の時計を読み直して出す

#![no_std]
#![no_main]

use vibe_rt::time::DateTime;
use vibe_rt::{eprintln, println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    match sys::cmos_time() {
        Ok(secs) => {
            println!("cmos: {}", DateTime::from_unix(secs as u64));
            0
        }
        Err(raw) => {
            eprintln!("hwclock: invalid time {raw:02x?}");
            1
        }
    }
}
