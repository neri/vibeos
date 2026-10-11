//! 画像ビューア: BMP を全画面で表示し、キーを押すと戻る
//!
//! 8 ビットの画像は画像のパレットをそのまま使い（正確な色）、24/32 ビットは標準パレットに
//! 誤差拡散で変換する。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_rt::gfx::{HEIGHT, Screen, WIDTH};
use vibe_rt::gfx_lib::bmp::Bmp;
use vibe_rt::println;

vibe_rt::entry!(main);

fn main() -> i32 {
    let Some(path) = vibe_rt::args().nth(1) else {
        println!("usage: view file.bmp");
        return 2;
    };
    let data = match vibe_rt::read_file(path) {
        Ok(d) => d,
        Err(e) => {
            println!("view: {path}: {}", vibe_rt::abi::err::name(e));
            return 1;
        }
    };
    let bmp = match Bmp::parse(&data) {
        Ok(b) => b,
        Err(e) => {
            println!("view: {path}: {e}");
            return 1;
        }
    };
    let (pixels, palette): (Vec<u8>, Option<Vec<[u8; 3]>>) = if bmp.is_indexed() {
        let px = (0..bmp.height).flat_map(|y| (0..bmp.width).map(move |x| (x, y))).map(|(x, y)| bmp.index(x, y));
        (px.collect(), Some(bmp.colors().iter().map(|c| [c[0] >> 2, c[1] >> 2, c[2] >> 2]).collect()))
    } else {
        (bmp.to_standard_palette(), None)
    };
    println!("view: showing {}x{}", bmp.width, bmp.height);

    let Some(mut screen) = Screen::open(false) else {
        println!("view: cannot open the screen");
        return 1;
    };
    if let Some(p) = &palette {
        screen.set_palette(0, p);
    }
    let x = (WIDTH as i32 - bmp.width as i32).max(0) / 2;
    let y = (HEIGHT as i32 - bmp.height as i32).max(0) / 2;
    screen.surface().blit(x, y, &pixels, bmp.width as usize, None);
    screen.present();

    vibe_rt::wait_key();
    drop(screen);
    let kind = if bmp.is_indexed() { "8-bit" } else { "true color" };
    println!("view: closed ({}x{} {kind})", bmp.width, bmp.height);
    0
}
