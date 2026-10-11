//! お絵かき: マウスの左ボタンで線を描く。上の色の並びをクリックで色を選ぶ。
//! C で全部消す、Esc で終わる。

use vibe_gfx::palette;

use crate::drivers::keyboard;
use crate::drivers::vga::{self, DisplayId, SCREEN};
use crate::input;

const SWATCH: i32 = 20;
const TOOLBAR_H: i32 = 12;
const CANVAS_BG: u8 = palette::WHITE;

pub fn run() {
    let id = {
        let mut s = SCREEN.lock();
        let id = s.create_display();
        draw_ui(&mut s, id, palette::BLACK);
        s.set_foreground(id);
        s.set_cursor_visible(true);
        s.present();
        id
    };
    input::clear_mouse_events();
    klog!("paint: started");

    let mut color = palette::BLACK;
    let mut last: Option<(i32, i32)> = None;
    'main: loop {
        input::wait();
        while let Some(k) = keyboard::try_read_event() {
            if !k.pressed {
                continue;
            }
            match k.ch {
                Some('\x1b') => break 'main,
                Some('c') | Some('C') => {
                    let mut s = SCREEN.lock();
                    draw_ui(&mut s, id, color);
                    s.present();
                }
                _ => {}
            }
        }
        while let Some(m) = input::poll_mouse() {
            if !m.left {
                last = None;
                continue;
            }
            let mut s = SCREEN.lock();
            if m.y < TOOLBAR_H {
                // 色を選ぶ
                let i = m.x / SWATCH;
                if (0..16).contains(&i) {
                    color = i as u8;
                    draw_toolbar(&mut s, id, color);
                }
                last = None;
            } else {
                let (x0, y0) = last.unwrap_or((m.x, m.y));
                if let Some(mut surf) = s.display_surface(id) {
                    surf.line(x0, y0.max(TOOLBAR_H), m.x, m.y, color);
                }
                let (lo, hi) = (y0.min(m.y).max(0) as usize, y0.max(m.y) as usize + 1);
                s.mark_display_dirty(id, lo, hi);
                last = Some((m.x, m.y));
            }
            s.present();
        }
    }

    let mut s = SCREEN.lock();
    s.set_cursor_visible(false);
    s.destroy_display(id);
    s.present();
    klog!("paint: closed");
}

fn draw_ui(s: &mut vga::Screen, id: DisplayId, color: u8) {
    if let Some(mut surf) = s.display_surface(id) {
        surf.clear(CANVAS_BG);
    }
    draw_toolbar(s, id, color);
    s.mark_display_dirty(id, 0, vga::HEIGHT);
}

fn draw_toolbar(s: &mut vga::Screen, id: DisplayId, color: u8) {
    let Some(mut surf) = s.display_surface(id) else { return };
    surf.fill_rect(0, 0, vga::WIDTH as i32, TOOLBAR_H, palette::DARK_GRAY);
    for i in 0..16 {
        surf.fill_rect(i * SWATCH + 1, 1, SWATCH - 2, TOOLBAR_H - 2, i as u8);
        if i as u8 == color {
            surf.rect(i * SWATCH, 0, SWATCH, TOOLBAR_H, palette::YELLOW);
        }
    }
    s.mark_display_dirty(id, 0, TOOLBAR_H as usize);
}
