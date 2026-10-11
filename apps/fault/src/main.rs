//! テスト用: わざと不正なことをするアプリ。カーネルが落ちずにこのプロセスだけを止めることを確かめる
//!
//! fault null | kernel | cli | div0 | stack | badptr | spin | exit N

#![no_std]
#![no_main]

use core::arch::asm;

use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let mut args = vibe_rt::args().skip(1);
    match args.next().unwrap_or("") {
        "null" => {
            // SAFETY: わざと 0 番地を読む
            let v = unsafe { core::ptr::read_volatile(core::ptr::null::<u32>()) };
            println!("read {v} (should not happen)");
        }
        "kernel" => {
            // SAFETY: わざとカーネルのメモリに書く
            unsafe { core::ptr::write_volatile(0x0010_0000 as *mut u32, 0) };
            println!("wrote kernel memory (should not happen)");
        }
        "cli" => {
            // SAFETY: わざと特権命令を使う
            unsafe { asm!("cli") };
            println!("cli succeeded (should not happen)");
        }
        "div0" => {
            // SAFETY: わざと 0 で割る
            unsafe { asm!("div {0}", in(reg) 0u32, inout("eax") 1u32 => _, inout("edx") 0u32 => _) };
        }
        "stack" => {
            println!("recursing...");
            recurse(0);
        }
        "badptr" => {
            // カーネルのメモリを指すポインタを渡しても、カーネルは読まずにエラーを返す
            let r = sys::syscall(vibe_rt::abi::nr::WRITE, 1, 0x0010_0000, 16, 0, 0);
            println!("write(kernel pointer) = {r}");
            let r = sys::syscall(vibe_rt::abi::nr::WRITE, 1, 0xBFFF_0000, 0x0002_0000, 0, 0);
            println!("write(beyond stack) = {r}");
            let r = sys::syscall(999, 0, 0, 0, 0, 0);
            println!("syscall 999 = {r}");
        }
        "spin" => {
            println!("spinning (Ctrl+C to stop)...");
            loop {
                core::hint::spin_loop();
            }
        }
        "exit" => return args.next().and_then(|s| s.parse().ok()).unwrap_or(0),
        other => println!("usage: fault null|kernel|cli|div0|stack|badptr|spin|exit N (got {other:?})"),
    }
    0
}

#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(depth: u32) -> u32 {
    let buf = [depth as u8; 1024];
    let r = recurse(depth + 1);
    core::hint::black_box(&buf)[0] as u32 + r
}
