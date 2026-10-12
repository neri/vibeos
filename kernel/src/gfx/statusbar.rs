//! ステータスバー（専用のスレッドが 1 秒ごとに描き直す）。全画面では一番上の行に自分で描き、
//! デスクトップでは上端のバーの右側に出す文字列を gui に渡す

use alloc::format;

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use crate::drivers::pit;
use crate::gui::SCREEN;
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
    let ip = crate::net::address();
    let desktop = SCREEN.lock().mode() == crate::gui::Mode::Desktop;
    // 時計が読めれば日時、読めなければ起動からの時間。320x200 で IP アドレスを出すときは時刻だけ（40 桁に収める）
    let when = match crate::drivers::rtc::now() {
        Some(t) if ip.is_some() && !desktop => format!("{:02}:{:02}", t.hour, t.minute),
        Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute),
        None => format!("up {}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60),
    };
    let right = format!("{when}  {}K T{} ", free_pages * 4, tasks);
    let mut s = SCREEN.lock();
    if desktop {
        // デスクトップのバーは gui が描く（メニュー・ウィンドウのボタンと並べる）
        // 長さが変わる数は幅をそろえ、アドレスの位置が動かないようにする
        let fixed = format!("{when}  {:>5}K T{:<2} ", free_pages * 4, tasks);
        let text = match ip {
            Some(ip) => format!("{ip}  {fixed}"),
            None => fixed,
        };
        s.set_status_text(&text);
        s.present();
        return;
    }
    let font = &FONT_8X8;
    s.draw_status_bar(|surf| {
        let w = surf.width() as i32;
        surf.fill_rect(0, 0, w, font.height as i32, BAR_BG);
        surf.draw_text(font, font.width as i32, 0, "VibeOS", palette::YELLOW, None);
        // ネットワークのアドレスは左側（右側は長さが変わるので、位置が動かないように）
        if let Some(ip) = ip {
            surf.draw_text(font, 8 * font.width as i32, 0, &format!("{ip}"), palette::LIGHT_CYAN, None);
        }
        let x = w - (right.len() * font.width) as i32;
        surf.draw_text(font, x, 0, &right, BAR_FG, None);
    });
    s.present();
}
