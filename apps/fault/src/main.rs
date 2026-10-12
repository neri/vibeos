//! テスト用: わざと不正なことをするアプリ。カーネルが落ちずにこのプロセスだけを止めることを確かめる
//!
//! fault null | kernel | cli | div0 | stack | badptr | spin | exit N
//!
//! パイプと起動の試験（カーネル内テストから使う。合っていれば 0 で終わる）:
//! fault pipe（読む側のいないパイプに書く → EXIT_PIPE で止まるはず）| fds（0〜2 だけを受け継いだ）|
//! argv A B（引数がちょうど A B で、環境変数 FOO がある）| spawnenv（VIBE=1 を足して checkenv を起動）|
//! checkenv（FOO と VIBE が親から届いた）
//!
//! シェルのパイプとリダイレクトの試験: upper（標準入力を大文字にして標準出力へ）|
//! stderr（標準出力と標準エラーに 1 行ずつ）

#![no_std]
#![no_main]

extern crate alloc;

use core::arch::asm;

use vibe_rt::{abi, println, sys};

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
        "pipe" => {
            let Ok((r, w)) = sys::pipe() else { return 2 };
            sys::close(r);
            let n = sys::write(w, b"nobody reads this");
            println!("write to a closed pipe = {n} (should not return)");
            return 1;
        }
        "fds" => {
            let ok = (0..3).all(|fd| sys::fd_kind(fd) > 0) && (3..32).all(|fd| sys::fd_kind(fd) < 0);
            println!("fault: fds {}", if ok { "ok" } else { "NG" });
            return if ok { 0 } else { 1 };
        }
        "argv" => {
            let got: alloc::vec::Vec<&str> = args.collect();
            let foo = vibe_rt::env::var("FOO");
            println!("fault: argv {got:?} FOO={foo:?}");
            return if got == ["a b", "c"] && foo.as_deref() == Some("bar baz") { 0 } else { 1 };
        }
        "spawnenv" => {
            vibe_rt::env::set_var("VIBE", "1");
            let pid = sys::spawn("fault", "checkenv");
            return if pid < 0 { 2 } else { sys::wait(pid) };
        }
        "checkenv" => {
            let (foo, vibe) = (vibe_rt::env::var("FOO"), vibe_rt::env::var("VIBE"));
            println!("fault: env FOO={foo:?} VIBE={vibe:?}");
            return if foo.as_deref() == Some("bar baz") && vibe.as_deref() == Some("1") { 0 } else { 1 };
        }
        "upper" => {
            let mut buf = [0u8; 256];
            loop {
                let n = sys::read(abi::STDIN, &mut buf);
                if n <= 0 {
                    return if n == 0 { 0 } else { 1 };
                }
                let chunk = &mut buf[..n as usize];
                chunk.make_ascii_uppercase();
                sys::write(abi::STDOUT, chunk);
            }
        }
        "stderr" => {
            sys::write(abi::STDOUT, b"fault: to stdout\n");
            sys::write(abi::STDERR, b"fault: to stderr\n");
        }
        other => println!("usage: fault null|kernel|cli|div0|stack|badptr|spin|exit N|pipe|fds|argv|spawnenv|checkenv|upper|stderr (got {other:?})"),
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
