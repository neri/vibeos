//! beep [Hz] [ms]: PC スピーカーで鳴らす（既定 880Hz・200ms）。鳴り終わるまで待つ

#![no_std]
#![no_main]

use vibe_rt::sys;

vibe_rt::entry!(main);

fn main() -> i32 {
    let mut args = vibe_rt::args().skip(1);
    let freq = args.next().and_then(|s| s.parse().ok()).unwrap_or(880);
    let ms = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);
    sys::sound(freq, ms);
    sys::sleep(ms);
    0
}
