//! マンデルブロ集合を描く（固定小数点で計算。ソフトウェア浮動小数点は遅いため）
//!
//! 320x200 のウィンドウ。矢印キーで移動、+ / - で拡大縮小、Esc か [x] で終わる。
//! 色は抜け出すまでの回数で 16 色を巡る（集合の内側は黒）

#![no_std]
#![no_main]

use vibe_rt::gfx::{Event, HEIGHT, WIDTH, Window};
use vibe_rt::gfx_lib::palette::*;
use vibe_rt::{abi, println, sys};

vibe_rt::entry!(main);

/// 固定小数点の小数部のビット数
const FRAC: u32 = 24;
const ONE: i64 = 1 << FRAC;
const MAX_ITER: u32 = 96;

/// 抜け出すまでの回数で巡る色（黒以外の 15 色を、青から暖色へ）
const COLORS: [u8; 15] = [
    BLUE, LIGHT_BLUE, CYAN, LIGHT_CYAN, GREEN, LIGHT_GREEN, YELLOW, BROWN, RED, LIGHT_RED, LIGHT_MAGENTA, MAGENTA,
    DARK_GRAY, LIGHT_GRAY, WHITE,
];

fn main() -> i32 {
    let Some(mut win) = Window::open(WIDTH, HEIGHT, "Mandel") else {
        println!("mandel: cannot open a window");
        return 1;
    };
    // 表示する範囲の中心と、1 ピクセルの大きさ（固定小数点）
    let (mut cx, mut cy) = (-ONE / 2, 0i64);
    let mut scale = 3 * ONE / WIDTH as i64;
    'main: loop {
        let start = sys::ticks();
        render(&mut win, cx, cy, scale);
        win.present();
        println!("mandel: rendered in {} ticks", sys::ticks() - start);
        let k = loop {
            match win.event(None) {
                Some(Event::Close) | None => break 'main,
                Some(Event::Key(k)) if k.pressed() => break k,
                _ => {}
            }
        };
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
    drop(win);
    0
}

fn render(win: &mut Window, cx: i64, cy: i64, scale: i64) {
    let mut s = win.surface();
    for y in 0..HEIGHT {
        let ci = cy + (y as i64 - HEIGHT as i64 / 2) * scale;
        for x in 0..WIDTH {
            let cr = cx + (x as i64 - WIDTH as i64 / 2) * scale;
            let n = escape_time(cr, ci);
            let c = if n == 0 { BLACK } else { COLORS[(n as usize - 1) / 2 % COLORS.len()] };
            s.put_pixel(x as i32, y as i32, c);
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
