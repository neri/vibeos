//! マンデルブロ集合を描く（固定小数点で計算。ソフトウェア浮動小数点は遅いため）
//!
//! 矢印キーで移動、+ / - で拡大縮小、Esc で終わる。

#![no_std]
#![no_main]

use vibe_rt::gfx::{HEIGHT, Screen, WIDTH};
use vibe_rt::{abi, println, sys};

vibe_rt::entry!(main);

/// 固定小数点の小数部のビット数
const FRAC: u32 = 24;
const ONE: i64 = 1 << FRAC;
const MAX_ITER: u32 = 96;

fn main() -> i32 {
    let Some(mut screen) = Screen::open(false) else {
        println!("mandel: cannot open the screen");
        return 1;
    };
    // 0 = 集合の内側（黒）、1〜255 = 抜け出すまでの回数で色を変える
    let mut pal = [[0u8; 3]; 256];
    for (i, c) in pal.iter_mut().enumerate().skip(1) {
        let t = i as u32 * 6 % 256;
        *c = match t / 64 {
            0 => [0, (t % 64) as u8, 63],
            1 => [(t % 64) as u8, 63, 63 - (t % 64) as u8],
            2 => [63, 63 - (t % 64) as u8, 0],
            _ => [63 - (t % 64) as u8, 0, (t % 64) as u8],
        };
    }
    screen.set_palette(0, &pal);

    // 表示する範囲の中心と、1 ピクセルの大きさ（固定小数点）
    let (mut cx, mut cy) = (-ONE / 2, 0i64);
    let mut scale = 3 * ONE / WIDTH as i64;
    loop {
        let start = sys::ticks();
        render(&mut screen, cx, cy, scale);
        screen.present();
        println!("mandel: rendered in {} ticks", sys::ticks() - start);
        let k = vibe_rt::wait_key();
        let step = scale * 32;
        match (k.key(), k.ch()) {
            (abi::key::ESCAPE, _) => break,
            (abi::key::LEFT, _) => cx -= step,
            (abi::key::RIGHT, _) => cx += step,
            (abi::key::UP, _) => cy -= step,
            (abi::key::DOWN, _) => cy += step,
            (_, Some('+' | '=')) => scale = (scale / 2).max(1),
            (_, Some('-')) => scale *= 2,
            _ => {}
        }
    }
    drop(screen);
    0
}

fn render(screen: &mut Screen, cx: i64, cy: i64, scale: i64) {
    let px = screen.pixels_mut();
    for y in 0..HEIGHT {
        let ci = cy + (y as i64 - HEIGHT as i64 / 2) * scale;
        for x in 0..WIDTH {
            let cr = cx + (x as i64 - WIDTH as i64 / 2) * scale;
            px[y * WIDTH + x] = escape_time(cr, ci);
        }
    }
}

/// z = z^2 + c を繰り返し、|z| > 2 になるまでの回数（色の番号にする）。抜けなければ 0
fn escape_time(cr: i64, ci: i64) -> u8 {
    let (mut zr, mut zi) = (0i64, 0i64);
    for i in 0..MAX_ITER {
        let zr2 = zr * zr >> FRAC;
        let zi2 = zi * zi >> FRAC;
        if zr2 + zi2 > 4 * ONE {
            return (i % 255 + 1) as u8;
        }
        zi = (2 * zr * zi >> FRAC) + ci;
        zr = zr2 - zi2 + cr;
    }
    0
}
