//! マウス: パケットを解釈して位置を求め、gui（ウィンドウ）に渡すスレッド
//!
//! カーソルの表示・ウィンドウの操作・中身への振り分けは gui が行う。

use vibe_keyboard::mouse::{MouseDecoder, MousePacket};

use crate::drivers::{mouse, vga};
use crate::sync::IrqMutex;
use crate::task;

/// マウスの位置（画面の座標）
static POS: IrqMutex<(i32, i32)> = IrqMutex::new((0, 0));

pub fn start_mouse() {
    *POS.lock() = ((vga::width() / 2) as i32, (vga::height() / 2) as i32);
    task::spawn("mouse", || {
        let mut decoder = MouseDecoder::new();
        loop {
            mouse::RAW_WAIT.wait_until(mouse::has_raw);
            while let Some(b) = mouse::pop_raw() {
                if let Some(p) = decoder.feed(b) {
                    apply(p);
                }
            }
        }
    });
}

fn apply(p: MousePacket) {
    let (x, y) = {
        let mut pos = POS.lock();
        pos.0 = (pos.0 + p.dx as i32).clamp(0, vga::width() as i32 - 1);
        pos.1 = (pos.1 + p.dy as i32).clamp(0, vga::height() as i32 - 1);
        *pos
    };
    let buttons = p.left as u8 | (p.right as u8) << 1 | (p.middle as u8) << 2;
    crate::gui::on_mouse(x, y, buttons);
}
