//! マルチタスクの見本（システムコール demo）: 3 つのカーネルスレッドが、呼んだプロセスの端末の
//! 右上にそれぞれの数を描き続ける（5 秒で終わる。1 つは休まず、ほかは速さを変えて休む）

use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use crate::drivers::pit;
use crate::gui::SCREEN;
use crate::task::{self, sched};

static RUNNING: AtomicBool = AtomicBool::new(false);

/// 始める。もう動いていれば false
pub fn start() -> bool {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return false;
    }
    let colors = [palette::LIGHT_RED, palette::LIGHT_GREEN, palette::LIGHT_CYAN];
    let tty = sched::current_tty();
    let ids: Vec<_> = (0..3)
        .map(|i| {
            task::spawn(&format!("counter{i}"), move || {
                let start = pit::ticks();
                let mut n = 0u32;
                while pit::ticks().wrapping_sub(start) < 5 * pit::TICK_HZ {
                    n += 1;
                    let text = format!("T{i}:{n:6}");
                    let mut s = SCREEN.lock();
                    let Some((w, _)) = s.window_size(tty) else { break };
                    // 端末の右上に並べる
                    let x = (w - text.len() * FONT_8X8.width) as i32;
                    let y = (i * FONT_8X8.height) as i32;
                    if let Some(mut px) = s.window_pixels(tty) {
                        px.draw_text8(x, y, &text, colors[i], Some(palette::BLACK));
                    }
                    s.mark_window_dirty(tty, y as usize, y as usize + FONT_8X8.height);
                    s.present();
                    drop(s);
                    if i > 0 {
                        pit::sleep_ms(10 * i as u32);
                    }
                }
            })
        })
        .collect();
    task::spawn("demo-wait", move || {
        for id in ids {
            task::join(id);
        }
        RUNNING.store(false, Ordering::Relaxed);
        klog!("demo finished");
    });
    true
}
