//! お絵かき: マウスの左ボタンで線を描く。上の色の並びをクリックで色を選ぶ。
//! C で全部消す、Esc で終わる。

#![no_std]
#![no_main]

use vibe_rt::gfx::{HEIGHT, Screen, WIDTH};
use vibe_rt::gfx_lib::palette;
use vibe_rt::{abi, println, sys};

vibe_rt::entry!(main);

const SWATCH: i32 = 20;
const TOOLBAR_H: i32 = 12;
const CANVAS_BG: u8 = palette::WHITE;

fn main() -> i32 {
    let Some(mut screen) = Screen::open(true) else {
        println!("paint: cannot open the screen");
        return 1;
    };
    let mut color = palette::BLACK;
    clear(&mut screen, color);
    screen.present();
    println!("paint: started");

    let mut last: Option<(i32, i32)> = None;
    'main: loop {
        let mut changed = false;
        while let Some(k) = sys::getkey(true) {
            if !k.pressed() {
                continue;
            }
            match (k.key(), k.ch()) {
                (abi::key::ESCAPE, _) => break 'main,
                (_, Some('c' | 'C')) => {
                    clear(&mut screen, color);
                    changed = true;
                }
                _ => {}
            }
        }
        while let Some(m) = sys::mouse() {
            if m.buttons & 1 == 0 {
                last = None;
                continue;
            }
            if m.y < TOOLBAR_H {
                let i = m.x / SWATCH;
                if (0..16).contains(&i) {
                    color = i as u8;
                    toolbar(&mut screen, color);
                    changed = true;
                }
                last = None;
            } else {
                let (x0, y0) = last.unwrap_or((m.x, m.y));
                screen.surface().line(x0, y0.max(TOOLBAR_H), m.x, m.y, color);
                last = Some((m.x, m.y));
                changed = true;
            }
        }
        if changed {
            screen.present();
        } else {
            sys::sleep(10);
        }
    }
    drop(screen);
    println!("paint: closed");
    0
}

fn clear(screen: &mut Screen, color: u8) {
    screen.surface().fill_rect(0, 0, WIDTH as i32, HEIGHT as i32, CANVAS_BG);
    toolbar(screen, color);
}

fn toolbar(screen: &mut Screen, color: u8) {
    let mut s = screen.surface();
    s.fill_rect(0, 0, WIDTH as i32, TOOLBAR_H, palette::DARK_GRAY);
    for i in 0..16 {
        s.fill_rect(i * SWATCH + 1, 1, SWATCH - 2, TOOLBAR_H - 2, i as u8);
        if i as u8 == color {
            s.rect(i * SWATCH, 0, SWATCH, TOOLBAR_H, palette::YELLOW);
        }
    }
}
