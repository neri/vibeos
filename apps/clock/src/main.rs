//! 時計のウィンドウ（アナログの針と日時）。[x] か Esc で終わる

#![no_std]
#![no_main]

use vibe_rt::abi::key;
use vibe_rt::font::FONT_8X8;
use vibe_rt::gfx::{Event, Window};
use vibe_rt::gfx_lib::palette;
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

const W: usize = 136;
const H: usize = 156;
const R: i32 = 60;

/// sin(6° × i) × 1000（i = 0..=15, 4 分の 1 周）
const SIN: [i32; 16] = [0, 105, 208, 309, 407, 500, 588, 669, 743, 809, 866, 914, 951, 978, 995, 1000];

/// 文字盤の 60 等分の i 番目（12 時が 0、時計回り）の向き (dx, dy) × 1000
fn dir(i: i32) -> (i32, i32) {
    let i = i.rem_euclid(60);
    let s = |k: i32| -> i32 {
        let k = k.rem_euclid(60);
        match k {
            0..=15 => SIN[k as usize],
            16..=30 => SIN[(30 - k) as usize],
            31..=45 => -SIN[(k - 30) as usize],
            _ => -SIN[(60 - k) as usize],
        }
    };
    (s(i), -s(i + 15))
}

fn main() -> i32 {
    let Some(mut win) = Window::open(W, H, "Clock") else {
        println!("clock: cannot open a window");
        return 1;
    };
    println!("clock: started");
    loop {
        draw(&mut win, sys::time());
        win.present();
        // 1 秒ごとに描き直す。その間にイベントが来れば処理する
        match win.event(Some(1000)) {
            Some(Event::Close) => break,
            Some(Event::Key(k)) if k.pressed() && k.key() == key::ESCAPE => break,
            _ => {}
        }
    }
    drop(win);
    println!("clock: closed");
    0
}

fn draw(win: &mut Window, now: u32) {
    let t = vibe_rt::time::DateTime::from_unix(now as u64);
    let mut s = win.surface();
    s.clear(palette::WHITE);
    let (cx, cy) = (W as i32 / 2, R + 8);
    // 文字盤: 5 分ごとに太い目盛り
    for i in 0..60 {
        let (dx, dy) = dir(i);
        let inner = if i % 5 == 0 { R - 8 } else { R - 3 };
        s.line(cx + dx * inner / 1000, cy + dy * inner / 1000, cx + dx * R / 1000, cy + dy * R / 1000, palette::DARK_GRAY);
    }
    let hand = |s: &mut vibe_rt::gfx_lib::Surface, pos: i32, len: i32, color: u8, thick: bool| {
        let (dx, dy) = dir(pos);
        let (x, y) = (cx + dx * len / 1000, cy + dy * len / 1000);
        s.line(cx, cy, x, y, color);
        if thick {
            s.line(cx + 1, cy, x + 1, y, color);
            s.line(cx, cy + 1, x, y + 1, color);
        }
    };
    let hour_pos = (t.hour as i32 % 12) * 5 + t.minute as i32 / 12;
    hand(&mut s, hour_pos, R * 5 / 10, palette::BLACK, true);
    hand(&mut s, t.minute as i32, R * 8 / 10, palette::BLACK, true);
    hand(&mut s, t.second as i32, R * 9 / 10, palette::RED, false);
    s.fill_rect(cx - 2, cy - 2, 5, 5, palette::BLACK);
    let text = vibe_rt::format_fixed::<20>(format_args!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second));
    let x = (W as i32 - text.len() as i32 * FONT_8X8.width as i32) / 2;
    s.draw_text(&FONT_8X8, x, H as i32 - 20, &text, palette::BLUE, None);
    let date = vibe_rt::format_fixed::<20>(format_args!("{:04}-{:02}-{:02}", t.year, t.month, t.day));
    let x = (W as i32 - date.len() as i32 * FONT_8X8.width as i32) / 2;
    s.draw_text(&FONT_8X8, x, H as i32 - 10, &date, palette::DARK_GRAY, None);
}
