//! ステータスバー（専用のスレッドが 1 秒ごとに作り直す）。デスクトップの上端のバーの右側に出す文字列を gui に渡す
//! （アドレス・日時か起動からの時間・空きメモリ・タスク数）

use alloc::format;

use crate::drivers::pit;
use crate::gui::SCREEN;
use crate::mm::pmm;
use crate::task::{self, sched};

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
    // 時計が読めれば日時、読めなければ起動からの時間
    let when = match crate::drivers::rtc::now() {
        Some(t) => format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute),
        None => format!("up {}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60),
    };
    // 長さが変わる数は幅をそろえ、アドレスの位置が動かないようにする
    let fixed = format!("{when}  {:>5}K T{:<2} ", free_pages * 4, tasks);
    let text = match ip {
        Some(ip) => format!("{ip}  {fixed}"),
        None => fixed,
    };
    let mut s = SCREEN.lock();
    s.set_status_text(&text);
    s.present();
}
