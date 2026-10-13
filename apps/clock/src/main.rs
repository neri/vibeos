//! 時計のウィンドウ（アナログの針と日時）。[x] か Esc で終わる

#![no_std]
#![no_main]

use vibe_rt::abi::key;
use vibe_rt::font::FONT_8X8;
use vibe_rt::gfx::{Event, Window32 as Window};
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
    use vibe_rt::gfx_lib::canvas::{Corner, rgb};
    use vibe_rt::gfx_lib::theme;
    let t = vibe_rt::time::DateTime::from_unix(now as u64);
    let mut c = win.canvas();
    c.clear(rgb(255, 255, 255));
    let (cx, cy) = (W as i32 / 2, R + 8);
    // 文字盤: 薄い灰色の円に、5 分ごとの太い目盛りと 1 分ごとの細い目盛り（なめらかな線。1/16 ピクセル単位）
    c.fill_round_rect(cx - R, cy - R, 2 * R, 2 * R, &Corner::new(R as usize), theme::SUBTLE, 255);
    let p = |v: i32| v * 16 + 8;
    let at = |pos: i32, len: i32| {
        let (dx, dy) = dir(pos);
        (p(cx) + dx * len * 16 / 1000, p(cy) + dy * len * 16 / 1000)
    };
    for i in 0..60 {
        let (major, inner) = if i % 5 == 0 { (true, R - 9) } else { (false, R - 5) };
        let (x0, y0) = at(i, inner);
        let (x1, y1) = at(i, R - 3);
        let (w, color) = if major { (32, theme::TEXT) } else { (16, rgb(186, 183, 172)) };
        c.line_aa(x0, y0, x1, y1, w, color);
    }
    let hand = |c: &mut vibe_rt::gfx_lib::canvas::Canvas, pos: i32, len: i32, width: i32, color| {
        let (x, y) = at(pos, len);
        c.line_aa(p(cx), p(cy), x, y, width, color);
    };
    let hour_pos = (t.hour as i32 % 12) * 5 + t.minute as i32 / 12;
    hand(&mut c, hour_pos, R * 5 / 10, 64, theme::TEXT);
    hand(&mut c, t.minute as i32, R * 8 / 10, 48, theme::TEXT);
    hand(&mut c, t.second as i32, R * 9 / 10, 20, theme::CLOSE_HOVER);
    c.fill_round_rect(cx - 3, cy - 3, 7, 7, &Corner::new(3), theme::CLOSE_HOVER, 255);
    let text = vibe_rt::format_fixed::<20>(format_args!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second));
    let date = vibe_rt::format_fixed::<20>(format_args!("{:04}-{:02}-{:02}", t.year, t.month, t.day));
    let center = |c: &mut vibe_rt::gfx_lib::canvas::Canvas, kind, top, s: &str, color| match vibe_rt::gfx::font(kind) {
        Some(f) => {
            let x = (W as i32 - f.measure(s)) / 2;
            c.draw_vtext(f, x, top, s, color);
        }
        None => {
            let x = (W as i32 - s.len() as i32 * FONT_8X8.width as i32) / 2;
            c.draw_text(&FONT_8X8, x, top + 4, s, color, None);
        }
    };
    center(&mut c, vibe_rt::abi::FONT_BOLD, cy + R + 4, &text, theme::TEXT);
    center(&mut c, vibe_rt::abi::FONT_UI, cy + R + 21, &date, theme::TEXT_DIM);
}
