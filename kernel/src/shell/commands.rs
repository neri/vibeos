//! シェルの内蔵コマンド
//!
//! 出力は `out!`・`outln!`（今のタスクの fd 1）、エラーは `errln!`（fd 2）。fd 1 が端末なら今までどおり
//! 画面とシリアルに出る。書けなかったとき（パイプの先が閉じた・止めるよう頼まれた）は `?` で抜ける。
//! パイプの中では短い間だけのカーネルスレッドで動く（`shell::run_pipeline`）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use vibe_abi::err;

use super::{Out, Shell, out_bytes, stdout_is_console};
use crate::drivers::{pit, power};
use crate::gui::SCREEN;
use crate::task::{self, sched};
use crate::{fs, io};

pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    pub run: fn(&mut Shell, &[&str]) -> Out,
}

const COMMANDS: &[Command] = &[
    Command { name: "help", usage: "help [command]", help: "show commands", run: help },
    Command { name: "clear", usage: "clear", help: "clear the screen", run: clear },
    Command { name: "echo", usage: "echo [text...]", help: "print text", run: echo },
    Command { name: "ls", usage: "ls [dir]", help: "list a directory", run: ls },
    Command { name: "cat", usage: "cat [file...]", help: "print files (or standard input)", run: cat },
    Command { name: "write", usage: "write file text...", help: "write text to a file (replaces it)", run: write },
    Command { name: "append", usage: "append file text...", help: "add a line of text to a file", run: append },
    Command { name: "cp", usage: "cp from to", help: "copy a file", run: cp },
    Command { name: "rm", usage: "rm path", help: "remove a file or an empty directory", run: rm },
    Command { name: "mv", usage: "mv from to", help: "rename or move a file or directory", run: mv },
    Command { name: "mkdir", usage: "mkdir dir", help: "make a directory", run: mkdir },
    Command { name: "sync", usage: "sync", help: "save changes to the floppy disk", run: sync },
    Command { name: "cd", usage: "cd [dir]", help: "change directory", run: cd },
    Command { name: "pwd", usage: "pwd", help: "print current directory", run: pwd },
    Command { name: "hwclock", usage: "hwclock", help: "read the CMOS clock", run: hwclock },
    Command { name: "sleep", usage: "sleep [ms]", help: "sleep (default 500ms, checks timing)", run: sleep },
    Command { name: "history", usage: "history", help: "command history", run: history },
    Command { name: "term", usage: "term", help: "open a new terminal (Alt+F1-F4 to switch)", run: term },
    Command {
        name: "wallpaper",
        usage: "wallpaper file.bmp [tile|center] | none",
        help: "set the desktop background image",
        run: wallpaper,
    },
    Command { name: "demo", usage: "demo", help: "3 threads draw counters for 5s", run: demo },
    Command { name: "beep", usage: "beep [hz] [ms]", help: "sound the PC speaker", run: beep },
    Command { name: "reboot", usage: "reboot", help: "restart the computer", run: reboot },
    Command { name: "divzero", usage: "divzero", help: "test: divide by zero", run: divzero },
    Command { name: "nullptr", usage: "nullptr", help: "test: read address 0", run: nullptr },
    Command { name: "overflow", usage: "overflow", help: "test: kernel stack overflow", run: overflow },
    Command { name: "panic", usage: "panic", help: "test: kernel panic", run: panic_cmd },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

fn help(_: &mut Shell, args: &[&str]) -> Out {
    if let Some(name) = args.get(1) {
        match find(name) {
            Some(c) => outln!("{}\n  {}", c.usage, c.help),
            None => errln!("help: no such command: {name}"),
        }
        return Ok(());
    }
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    // 端末の幅に収まるように並べる
    let width = crate::gfx::console::size().map_or(40, |(cols, _)| cols);
    let mut line = String::new();
    for n in names {
        if line.len() + n.len() + 1 > width - 2 {
            outln!("{line}");
            line.clear();
        }
        line.push_str(n);
        line.push(' ');
    }
    outln!("{line}");
    outln!("'help <command>' for details. Other commands are programs in /BIN");
    Ok(())
}

fn clear(_: &mut Shell, _: &[&str]) -> Out {
    out!("\x1b[2J\x1b[H");
    Ok(())
}

fn echo(_: &mut Shell, args: &[&str]) -> Out {
    outln!("{}", args[1..].join(" "));
    Ok(())
}

fn ls(_: &mut Shell, args: &[&str]) -> Out {
    let path = args.get(1).copied().unwrap_or(".");
    // 色は端末に出すときだけ
    let color = stdout_is_console();
    match fs::read_dir(path) {
        Ok(entries) => {
            for e in entries {
                let when = vibe_time::DateTime::from_fat(e.modified.0, e.modified.1)
                    .map(|t| format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute))
                    .unwrap_or_default();
                if e.is_dir() && color {
                    outln!(" \x1b[94m{:12}\x1b[0m   <DIR> {when}", e.name);
                } else if e.is_dir() {
                    outln!(" {:12}   <DIR> {when}", e.name);
                } else {
                    outln!(" {:12} {:7} {when}", e.name, e.size);
                }
            }
        }
        Err(e) => errln!("ls: {path}: {e}"),
    }
    Ok(())
}

fn cat(_: &mut Shell, args: &[&str]) -> Out {
    if args.len() < 2 {
        return copy_fd(0, false);
    }
    for path in &args[1..] {
        let fd = match fs::open(path) {
            Ok(fd) => fd,
            Err(e) => {
                errln!("cat: {path}: {e}");
                continue;
            }
        };
        let r = copy_fd(fd, true);
        let _ = fs::close(fd);
        r?;
    }
    Ok(())
}

/// fd を終わりまで読んで fd 1 に書く。端末に出すときは表示できない文字を '.' にし、
/// `newline` なら最後に改行が無ければ足す
fn copy_fd(fd: fs::Fd, newline: bool) -> Out {
    let console = stdout_is_console();
    let mut buf = [0u8; 256];
    let mut last = b'\n';
    loop {
        let n = match io::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err::EINTR) => return Err(err::EINTR),
            Err(e) => {
                errln!("cat: {}", err::name(e));
                break;
            }
        };
        let data = &mut buf[..n];
        if console {
            for b in data.iter_mut() {
                if *b != b'\n' && !(0x20..0x7F).contains(b) {
                    *b = b'.';
                }
            }
        }
        out_bytes(data)?;
        last = data[n - 1];
    }
    if newline && console && last != b'\n' {
        outln!();
    }
    Ok(())
}

fn write(_: &mut Shell, args: &[&str]) -> Out {
    let Some(path) = args.get(1) else {
        errln!("usage: write file text...");
        return Ok(());
    };
    let mut text = args[2..].join(" ");
    text.push('\n');
    if let Err(e) = fs::write_file(path, text.as_bytes()) {
        errln!("write: {path}: {e}");
    }
    Ok(())
}

fn append(_: &mut Shell, args: &[&str]) -> Out {
    let Some(path) = args.get(1) else {
        errln!("usage: append file text...");
        return Ok(());
    };
    let mut text = args[2..].join(" ");
    text.push('\n');
    let r = fs::open_with(path, fs::OPEN_WRITE | fs::OPEN_CREATE | fs::OPEN_APPEND).and_then(|fd| {
        let r = fs::write(fd, text.as_bytes());
        fs::close(fd)?;
        r
    });
    if let Err(e) = r {
        errln!("append: {path}: {e}");
    }
    Ok(())
}

fn cp(_: &mut Shell, args: &[&str]) -> Out {
    let [_, from, to] = args else {
        errln!("usage: cp from to");
        return Ok(());
    };
    match fs::read_file(from) {
        Ok(data) => {
            if let Err(e) = fs::write_file(to, &data) {
                errln!("cp: {to}: {e}");
            }
        }
        Err(e) => errln!("cp: {from}: {e}"),
    }
    Ok(())
}

fn mv(_: &mut Shell, args: &[&str]) -> Out {
    let [_, from, to] = args else {
        errln!("usage: mv from to");
        return Ok(());
    };
    if let Err(e) = fs::rename(from, to) {
        errln!("mv: {from}: {e}");
    }
    Ok(())
}

fn rm(_: &mut Shell, args: &[&str]) -> Out {
    let Some(path) = args.get(1) else {
        errln!("usage: rm path");
        return Ok(());
    };
    if let Err(e) = fs::remove(path) {
        errln!("rm: {path}: {e}");
    }
    Ok(())
}

fn mkdir(_: &mut Shell, args: &[&str]) -> Out {
    let Some(path) = args.get(1) else {
        errln!("usage: mkdir dir");
        return Ok(());
    };
    if let Err(e) = fs::mkdir(path) {
        errln!("mkdir: {path}: {e}");
    }
    Ok(())
}

fn sync(_: &mut Shell, _: &[&str]) -> Out {
    match fs::sync() {
        Ok(0) => outln!("sync: nothing to save"),
        Ok(n) => outln!("sync: saved {n} sectors"),
        Err(e) => errln!("sync: {e}"),
    }
    Ok(())
}

fn cd(_: &mut Shell, args: &[&str]) -> Out {
    let path = args.get(1).copied().unwrap_or("/");
    if let Err(e) = fs::chdir(path) {
        errln!("cd: {path}: {e}");
    }
    Ok(())
}

fn pwd(_: &mut Shell, _: &[&str]) -> Out {
    outln!("{}", fs::cwd());
    Ok(())
}

fn hwclock(_: &mut Shell, _: &[&str]) -> Out {
    match crate::drivers::rtc::read_cmos() {
        Ok(t) => outln!("cmos: {t}"),
        Err(raw) => errln!("hwclock: invalid time {raw:02x?}"),
    }
    Ok(())
}

fn sleep(_: &mut Shell, args: &[&str]) -> Out {
    let ms: u32 = match args.get(1).map(|s| s.parse()) {
        None => 500,
        Some(Ok(ms)) => ms,
        Some(Err(_)) => {
            errln!("usage: sleep [ms]");
            return Ok(());
        }
    };
    let start = pit::ticks();
    pit::sleep_ms(ms);
    let elapsed = pit::ticks().wrapping_sub(start);
    let expected = (ms * pit::TICK_HZ).div_ceil(1000);
    let ok = elapsed >= expected && elapsed <= expected + expected / 5 + 2;
    outln!("slept {} ticks: {}", elapsed, if ok { "sleep ok" } else { "sleep NG" });
    Ok(())
}

fn wallpaper(_: &mut Shell, args: &[&str]) -> Out {
    use vibe_gfx::wallpaper::parse_setting;
    match args.get(1..) {
        Some(["none"]) => crate::gfx::wallpaper::clear(),
        Some(rest) if !rest.is_empty() => match parse_setting(&rest.join(" ")) {
            Some((path, fit)) => {
                if let Err(e) = crate::gfx::wallpaper::load(path, fit) {
                    errln!("wallpaper: {path}: {e}");
                }
            }
            None => errln!("usage: wallpaper file.bmp [tile|center] | none"),
        },
        _ => errln!("usage: wallpaper file.bmp [tile|center] | none"),
    }
    Ok(())
}

fn term(_: &mut Shell, _: &[&str]) -> Out {
    match crate::gfx::console::open_terminal(None, None) {
        Some(_) => {}
        None => errln!("term: too many terminals"),
    }
    Ok(())
}

fn history(shell: &mut Shell, _: &[&str]) -> Out {
    for (i, h) in shell.editor.history().enumerate() {
        outln!("{:3} {h}", i + 1);
    }
    Ok(())
}

static DEMO_RUNNING: AtomicBool = AtomicBool::new(false);

/// 3 つのスレッドが画面の右上にそれぞれのカウンタを描き続ける（5 秒で終わる）
fn demo(_: &mut Shell, _: &[&str]) -> Out {
    if DEMO_RUNNING.swap(true, Ordering::Relaxed) {
        errln!("demo is already running");
        return Ok(());
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
                    if let Some(mut surf) = s.window_surface(tty) {
                        surf.draw_text(&FONT_8X8, x, y, &text, colors[i], Some(palette::BLACK));
                    }
                    s.mark_window_dirty(tty, y as usize, y as usize + FONT_8X8.height);
                    s.present();
                    drop(s);
                    // スレッドごとに速さを変える（1 つは休まない）
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
    outln!("demo started");
    Ok(())
}

fn beep(_: &mut Shell, args: &[&str]) -> Out {
    let freq = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(880);
    let ms = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    crate::drivers::speaker::play(freq, ms);
    pit::sleep_ms(ms);
    Ok(())
}

fn reboot(_: &mut Shell, _: &[&str]) -> Out {
    outln!("rebooting...");
    power::reboot();
}

fn divzero(_: &mut Shell, _: &[&str]) -> Out {
    // Rust の割り算はゼロ除算を事前に検査して panic するので、CPU の例外を起こすには div 命令を直接使う
    // SAFETY: わざと #DE を起こす（例外ハンドラの確認用）
    unsafe { asm!("div {0}", in(reg) 0u32, inout("eax") 1u32 => _, inout("edx") 0u32 => _) };
    Ok(())
}

fn nullptr(_: &mut Shell, _: &[&str]) -> Out {
    // SAFETY: わざと 0 番地を読んでページフォルトを起こす（ページングの確認用）
    let v = unsafe { core::ptr::read_volatile(core::ptr::null::<u32>()) };
    outln!("read {v:#x} from address 0 (should not happen)");
    Ok(())
}

fn overflow(_: &mut Shell, _: &[&str]) -> Out {
    outln!("overflowing the stack...");
    recurse(0);
    Ok(())
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

fn panic_cmd(_: &mut Shell, _: &[&str]) -> Out {
    panic!("panic requested from the shell");
}



