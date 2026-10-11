//! 画面の一番上の行のステータスバー（専用のスレッドが 1 秒ごとに描き直す）

use alloc::format;

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use crate::drivers::pit;
use crate::drivers::vga::{self, SCREEN};
use crate::mm::pmm;
use crate::task::{self, sched};

const BAR_BG: u8 = palette::BLUE;
const BAR_FG: u8 = palette::WHITE;

pub fn start() {
    task::spawn("statusbar", || {
        loop {
            draw();
            pit::sleep_ms(1000);
        }
    });
}

fn draw() {
    let secs = pit::ticks() / pit::TICK_HZ;
    let (free_pages, _) = pmm::stats();
    let tasks = sched::tasks().len();
    // 時計が読めれば日時、読めなければ起動からの時間
    let when = match crate::drivers::rtc::now() {
        Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute),
        None => format!("up {}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60),
    };
    let right = format!("{when}  {}K T{} ", free_pages * 4, tasks);
    let font = &FONT_8X8;
    let mut s = SCREEN.lock();
    {
        let mut surf = s.surface();
        surf.fill_rect(0, 0, vga::WIDTH as i32, font.height as i32, BAR_BG);
        surf.draw_text(font, font.width as i32, 0, "VibeOS", palette::YELLOW, None);
        let x = vga::WIDTH - right.len() * font.width;
        surf.draw_text(font, x as i32, 0, &right, BAR_FG, None);
    }
    s.mark_dirty(0, font.height);
    s.present();
}
