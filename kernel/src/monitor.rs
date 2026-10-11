//! 仮のモニタ（Phase 7 でシェルに置き換える）
//!
//! キー入力を画面に出し、いくつかのテスト用コマンドを受け付ける。

use core::arch::asm;

use crate::drivers::{keyboard, pit};

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
        "help" => kprintln!("help ticks sleep divzero panic"),
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
        "panic" => panic!("panic requested from monitor"),
        other => kprintln!("unknown command: {other}"),
    }
}
