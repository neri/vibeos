//! マウス: パケットを解釈して位置を求め、gui（ウィンドウ）に渡すスレッド
//!
//! カーソルの表示・ウィンドウの操作・中身への振り分けは gui が行う。溜まったパケットの動きはまとめて渡す。

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
        let mut sent_buttons = 0u8;
        loop {
            mouse::RAW_WAIT.wait_until(mouse::has_raw);
            // 溜まっているパケットはまとめて渡す: 動きだけのものは最後の位置を 1 回だけ（gui は渡すたびに画面を
            // 描き直すので、1 つずつ渡すと大きく動かしたときに間に合わず、生のバイトのリングがあふれて動きが消える）。
            // ボタンが変わったら、それまでの動きと、変わったパケットを順に渡す（押した・離した位置は落とさない）
            let mut moved = None;
            while let Some(b) = mouse::pop_raw() {
                let Some(p) = decoder.feed(b) else { continue };
                let (x, y, buttons) = apply(p);
                if buttons == sent_buttons {
                    moved = Some((x, y));
                    continue;
                }
                if let Some((mx, my)) = moved.take() {
                    crate::gui::on_mouse(mx, my, sent_buttons);
                }
                crate::gui::on_mouse(x, y, buttons);
                sent_buttons = buttons;
            }
            if let Some((x, y)) = moved {
                crate::gui::on_mouse(x, y, sent_buttons);
            }
        }
    });
}

/// パケットの動きで位置を進め、(x, y, ボタン) を返す
fn apply(p: MousePacket) -> (i32, i32, u8) {
    let (x, y) = {
        let mut pos = POS.lock();
        pos.0 = (pos.0 + p.dx as i32).clamp(0, vga::width() as i32 - 1);
        pos.1 = (pos.1 + p.dy as i32).clamp(0, vga::height() as i32 - 1);
        *pos
    };
    let buttons = p.left as u8 | (p.right as u8) << 1 | (p.middle as u8) << 2;
    (x, y, buttons)
}
