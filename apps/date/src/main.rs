//! date: 今の日時（現地時刻）と曜日

#![no_std]
#![no_main]

use vibe_rt::time::DateTime;
use vibe_rt::{eprintln, println, sys};

vibe_rt::entry!(main);

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn main() -> i32 {
    let now = sys::time();
    if now == 0 {
        eprintln!("date: no clock");
        return 1;
    }
    let t = DateTime::from_unix(now as u64);
    println!("{} {}", t, DAYS[t.weekday() as usize]);
    0
}
