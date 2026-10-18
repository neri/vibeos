//! 時計のウィンドウ（アナログの針と日時）。[x] か Esc で終わる

#![no_std]
#![no_main]

use vibe_rt::abi::key;
use vibe_rt::gfx::{Event, Window};
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

const W: usize = 136;
const H: usize = 170;
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
    use vibe_rt::gfx_lib::palette::{BLACK, DARK_GRAY, WHITE};
    let t = vibe_rt::time::DateTime::from_unix(now as u64);
    let mut s = win.surface();
    s.clear(WHITE);
    let (cx, cy) = (W as i32 / 2, R + 8);
    // 文字盤（白黒の画面。docs/plans/mono.md）: 白い円に 2 ピクセルの黒い縁、5 分ごとの太い目盛りと 1 分ごとの点
    s.circle(cx, cy, R, BLACK);
    s.circle(cx, cy, R - 1, BLACK);
    let at = |pos: i32, len: i32| {
        let (dx, dy) = dir(pos);
        (cx + (dx * len + 500 * dx.signum()) / 1000, cy + (dy * len + 500 * dy.signum()) / 1000)
    };
    for i in 0..60 {
        if i % 5 == 0 {
            let (x0, y0) = at(i, R - 10);
            let (x1, y1) = at(i, R - 4);
            s.thick_line(x0, y0, x1, y1, 2, BLACK);
        } else {
            let (x, y) = at(i, R - 5);
            s.put_pixel(x, y, BLACK);
        }
    }
    let hand = |s: &mut vibe_rt::gfx_lib::Surface, pos: i32, len: i32, width: i32, color| {
        let (x, y) = at(pos, len);
        s.thick_line(cx, cy, x, y, width, color);
    };
    let hour_pos = (t.hour as i32 % 12) * 5 + t.minute as i32 / 12;
    hand(&mut s, hour_pos, R * 5 / 10, 3, BLACK);
    hand(&mut s, t.minute as i32, R * 8 / 10, 2, BLACK);
    hand(&mut s, t.second as i32, R * 9 / 10, 1, BLACK);
    s.fill_circle(cx, cy, 3, BLACK);
    let text = vibe_rt::format_fixed::<20>(format_args!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second));
    let date = vibe_rt::format_fixed::<20>(format_args!("{:04}-{:02}-{:02}", t.year, t.month, t.day));
    vibe_rt::gfx::draw_text16_centered(&mut s, 0, W as i32, cy + R + 4, &text, BLACK, true);
    vibe_rt::gfx::draw_text16_centered(&mut s, 0, W as i32, cy + R + 21, &date, DARK_GRAY, false);
}
