//! 仮のモニタ（Phase 7 でシェルに置き換える）
//!
//! キー入力を画面に出し、いくつかのテスト用コマンドを受け付ける。

use core::arch::asm;

use alloc::format;
use core::sync::atomic::{AtomicBool, Ordering};

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use crate::drivers::vga::{self, SCREEN};
use crate::drivers::{keyboard, pit};
use crate::mm::{heap, pmm};
use crate::task::{self, sched};

const LINE_MAX: usize = 36;

pub fn run() -> ! {
    kprintln!("type 'help' for commands");
    loop {
        kprint!("> ");
        let mut buf = [0u8; LINE_MAX];
        let len = read_line(&mut buf);
        let line = core::str::from_utf8(&buf[..len]).unwrap_or("");
        execute(line.trim());
    }
}

fn read_line(buf: &mut [u8]) -> usize {
    let mut len = 0;
    loop {
        match keyboard::read_char() {
            '\n' => {
                kprintln!();
                return len;
            }
            '\x08' => {
                if len > 0 {
                    len -= 1;
                    kprint!("\x08");
                }
            }
            c if c.is_ascii() && !c.is_ascii_control() && len < buf.len() => {
                buf[len] = c as u8;
                len += 1;
                kprint!("{c}");
            }
            _ => {}
        }
    }
}

fn execute(cmd: &str) {
    match cmd {
        "" => {}
        "help" => kprintln!("help ticks sleep mem ps demo\ndivzero nullptr overflow panic"),
        "ticks" => kprintln!("ticks: {}", pit::ticks()),
        "sleep" => {
            let start = pit::ticks();
            pit::sleep_ms(500);
            let elapsed = pit::ticks().wrapping_sub(start);
            let ok = (50..=60).contains(&elapsed);
            kprintln!("slept {} ticks: {}", elapsed, if ok { "sleep ok" } else { "sleep NG" });
        }
        "divzero" => {
            // Rust の割り算はゼロ除算を事前に検査して panic するので、CPU の例外を起こすには div 命令を直接使う
            // SAFETY: わざと #DE を起こす（例外ハンドラの確認用）
            unsafe { asm!("div {0}", in(reg) 0u32, inout("eax") 1u32 => _, inout("edx") 0u32 => _) };
        }
        "mem" => {
            let (free, total) = pmm::stats();
            kprintln!("pages: {} KB free / {} KB", free * 4, total * 4);
            let (hfree, htotal, blocks) = heap::stats();
            kprintln!("heap: {} / {} bytes free, {} blocks", hfree, htotal, blocks);
        }
        "nullptr" => {
            // SAFETY: わざと 0 番地を読んでページフォルトを起こす（ページングの確認用）
            let v = unsafe { core::ptr::read_volatile(core::ptr::null::<u32>()) };
            kprintln!("read {v:#x} from address 0 (should not happen)");
        }
        "ps" => {
            for t in sched::tasks() {
                kprintln!("{:3} {:10} {:?} {}", t.id, t.name, t.state, t.ticks);
            }
        }
        "demo" => start_demo(),
        "overflow" => {
            kprintln!("overflowing the stack...");
            recurse(0);
        }
        "panic" => panic!("panic requested from monitor"),
        other => kprintln!("unknown command: {other}"),
    }
}

static DEMO_RUNNING: AtomicBool = AtomicBool::new(false);

/// 3 つのスレッドが画面の右上にそれぞれのカウンタを描き続ける（5 秒で終わる）
fn start_demo() {
    if DEMO_RUNNING.swap(true, Ordering::Relaxed) {
        kprintln!("demo is already running");
        return;
    }
    let colors = [palette::LIGHT_RED, palette::LIGHT_GREEN, palette::LIGHT_CYAN];
    let ids: alloc::vec::Vec<_> = (0..3)
        .map(|i| {
            task::spawn(&format!("counter{i}"), move || {
                let start = pit::ticks();
                let mut n = 0u32;
                while pit::ticks().wrapping_sub(start) < 5 * pit::TICK_HZ {
                    n += 1;
                    let text = format!("T{i}:{n:6}");
                    let x = (vga::WIDTH - text.len() * FONT_8X8.width) as i32;
                    let y = (i * FONT_8X8.height) as i32;
                    {
                        let mut s = SCREEN.lock();
                        s.surface().draw_text(&FONT_8X8, x, y, &text, colors[i], Some(palette::BLACK));
                        s.mark_dirty(y as usize, y as usize + FONT_8X8.height);
                        s.present();
                    }
                    // スレッドごとに速さを変える（プリエンプションで均等に進むことも確かめられるよう、1 つは休まない）
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
        DEMO_RUNNING.store(false, Ordering::Relaxed);
        klog!("demo finished");
    });
    kprintln!("demo started");
}

/// スタックを使い切るまで再帰する（ガードページと二重フォルト処理の確認用）
#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(depth: u32) -> u32 {
    let buf = [depth as u8; 512];
    let r = recurse(depth + 1);
    // 再帰の後で buf を使うので、末尾呼び出しの最適化で消えない
    core::hint::black_box(&buf)[0] as u32 + r
}
