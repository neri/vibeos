//! アプリの内蔵コマンド（このプロセスの中で動く。終了コードを返す）
//!
//! 言語の内蔵コマンド（`echo`・`test`・`export`・`exit` など）は `vibe_shell::interp` にある。
//! 出力は標準出力、エラーは標準エラー。カーネルの中でしかできないもの（`reboot`・`beep`・`hwclock`・`demo`）は
//! アプリ。`wallpaper` は名前が 8 文字を超えて FAT の名前（8.3）にできないので、ここに置く（DECISIONS.md D-045）。
//! ファイルに書くのは `echo … >` と `>>`

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::abi::{self, DirEntry, OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE, TICK_HZ, err};
use vibe_rt::{eprintln, io, print, println, sys};

use crate::host::AppHost;

pub struct Builtin {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    pub run: fn(&mut AppHost, &[&str]) -> i32,
}

const BUILTINS: &[Builtin] = &[
    Builtin { name: "help", usage: "help [command]", help: "show commands", run: help },
    Builtin { name: "clear", usage: "clear", help: "clear the screen", run: clear },
    Builtin { name: "ls", usage: "ls [dir]", help: "list a directory", run: ls },
    Builtin { name: "cat", usage: "cat [file...]", help: "print files (or standard input)", run: cat },
    Builtin { name: "cp", usage: "cp from to", help: "copy a file", run: cp },
    Builtin { name: "rm", usage: "rm path", help: "remove a file or an empty directory", run: rm },
    Builtin { name: "mv", usage: "mv from to", help: "rename or move a file or directory", run: mv },
    Builtin { name: "mkdir", usage: "mkdir dir", help: "make a directory", run: mkdir },
    Builtin { name: "sync", usage: "sync", help: "save changes to the floppy disk", run: sync },
    Builtin { name: "cd", usage: "cd [dir]", help: "change directory", run: cd },
    Builtin { name: "pwd", usage: "pwd", help: "print current directory", run: pwd },
    Builtin { name: "sleep", usage: "sleep [ms]", help: "sleep (default 500ms, checks timing)", run: sleep },
    Builtin { name: "history", usage: "history", help: "command history", run: history },
    Builtin { name: "term", usage: "term", help: "open a new terminal (Alt+F1-F4 to switch)", run: term },
    Builtin {
        name: "wallpaper",
        usage: "wallpaper file.bmp [tile|center] | none",
        help: "set the desktop background image",
        run: wallpaper,
    },
];

pub fn find(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

fn help(_: &mut AppHost, args: &[&str]) -> i32 {
    let lang = vibe_shell::interp::BUILTINS;
    if let Some(name) = args.get(1) {
        let found = find(name).map(|b| (b.usage, b.help)).or_else(|| lang.iter().find(|b| b.0 == *name).map(|b| (b.1, b.2)));
        return match found {
            Some((usage, help)) => {
                println!("{usage}\n  {help}");
                0
            }
            None => {
                eprintln!("help: no such command: {name}");
                1
            }
        };
    }
    // 端末の幅に収まるように並べる
    let width = sys::term_size().map_or(40, |(cols, _)| cols);
    let mut line = String::new();
    for name in BUILTINS.iter().map(|b| b.name).chain(lang.iter().map(|b| b.0)) {
        if line.len() + name.len() + 1 > width - 2 {
            println!("{line}");
            line.clear();
        }
        line.push_str(name);
        line.push(' ');
    }
    println!("{line}");
    println!("'help <command>' for details. Other commands are programs in /BIN");
    0
}

fn clear(_: &mut AppHost, _: &[&str]) -> i32 {
    print!("\x1b[2J\x1b[H");
    0
}

/// ディレクトリの中身（入りきらなければ大きな入れ物で読み直す）
fn read_dir(path: &str) -> Result<Vec<DirEntry>, i32> {
    let mut cap = 64;
    loop {
        let mut v = alloc::vec![DirEntry::EMPTY; cap];
        let n = sys::readdir(path, &mut v);
        if n < 0 {
            return Err(-n);
        }
        if (n as usize) < cap {
            v.truncate(n as usize);
            return Ok(v);
        }
        cap *= 4;
    }
}

fn ls(_: &mut AppHost, args: &[&str]) -> i32 {
    let path = args.get(1).copied().unwrap_or(".");
    // 色は端末に出すときだけ
    let color = vibe_rt::isatty(abi::STDOUT);
    let entries = match read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ls: {path}: {}", err::name(e));
            return 1;
        }
    };
    for e in entries {
        let when = vibe_rt::time::DateTime::from_fat(e.date, e.time)
            .map(|t| format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute))
            .unwrap_or_default();
        let name = e.name();
        if e.is_dir != 0 && color {
            println!(" \x1b[94m{name:12}\x1b[0m   <DIR> {when}");
        } else if e.is_dir != 0 {
            println!(" {name:12}   <DIR> {when}");
        } else {
            println!(" {name:12} {:7} {when}", e.size);
        }
    }
    0
}

fn cat(_: &mut AppHost, args: &[&str]) -> i32 {
    if args.len() < 2 {
        return copy_fd(abi::STDIN, false);
    }
    let mut code = 0;
    for path in &args[1..] {
        let fd = sys::open(path);
        if fd < 0 {
            eprintln!("cat: {path}: {}", err::name(-fd));
            code = 1;
            continue;
        }
        code |= copy_fd(fd, true);
        sys::close(fd);
    }
    code
}

/// fd を終わりまで読んで標準出力に書く。端末に出すときは表示できない文字を '.' にし、
/// `newline` なら最後に改行が無ければ足す
fn copy_fd(fd: i32, newline: bool) -> i32 {
    let console = vibe_rt::isatty(abi::STDOUT);
    let mut buf = [0u8; 256];
    let mut last = b'\n';
    loop {
        let n = sys::read(fd, &mut buf);
        if n == 0 {
            break;
        }
        if n < 0 {
            eprintln!("cat: {}", err::name(-n));
            return 1;
        }
        let data = &mut buf[..n as usize];
        if console {
            for b in data.iter_mut() {
                if *b != b'\n' && !(0x20..0x7F).contains(b) {
                    *b = b'.';
                }
            }
        }
        if io::write_all(abi::STDOUT, data).is_err() {
            return 1;
        }
        last = data[data.len() - 1];
    }
    if newline && console && last != b'\n' {
        println!();
    }
    0
}

/// ファイルに書く（`flags` は OPEN_TRUNCATE か OPEN_APPEND）
fn write_to(path: &str, data: &[u8], flags: u32) -> Result<(), i32> {
    let fd = sys::open_with(path, OPEN_WRITE | OPEN_CREATE | flags);
    if fd < 0 {
        return Err(-fd);
    }
    let r = io::write_all(fd, data);
    sys::close(fd);
    r
}

fn cp(_: &mut AppHost, args: &[&str]) -> i32 {
    let [_, from, to] = args else {
        eprintln!("usage: cp from to");
        return 2;
    };
    let data = match vibe_rt::read_file(from) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cp: {from}: {}", err::name(e));
            return 1;
        }
    };
    match write_to(to, &data, OPEN_TRUNCATE) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("cp: {to}: {}", err::name(e));
            1
        }
    }
}

/// 1 つのパスを取るコマンド（rm・mkdir）
fn path_op(args: &[&str], usage: &str, op: fn(&str) -> i32) -> i32 {
    let Some(path) = args.get(1) else {
        eprintln!("usage: {usage}");
        return 2;
    };
    match op(path) {
        r if r < 0 => {
            eprintln!("{}: {path}: {}", args[0], err::name(-r));
            1
        }
        _ => 0,
    }
}

fn rm(_: &mut AppHost, args: &[&str]) -> i32 {
    path_op(args, "rm path", sys::remove)
}

fn mkdir(_: &mut AppHost, args: &[&str]) -> i32 {
    path_op(args, "mkdir dir", sys::mkdir)
}

fn mv(_: &mut AppHost, args: &[&str]) -> i32 {
    let [_, from, to] = args else {
        eprintln!("usage: mv from to");
        return 2;
    };
    match sys::rename(from, to) {
        r if r < 0 => {
            eprintln!("mv: {from}: {}", err::name(-r));
            1
        }
        _ => 0,
    }
}

fn sync(_: &mut AppHost, _: &[&str]) -> i32 {
    match sys::sync() {
        0 => println!("sync: nothing to save"),
        n if n > 0 => println!("sync: saved {n} sectors"),
        e => {
            let why = match -e {
                err::EROFS => "no floppy drive to write to",
                err::EIO => "floppy write error",
                e => err::name(e),
            };
            eprintln!("sync: {why}");
            return 1;
        }
    }
    0
}

fn cd(_: &mut AppHost, args: &[&str]) -> i32 {
    let path = args.get(1).copied().unwrap_or("/");
    match sys::chdir(path) {
        r if r < 0 => {
            eprintln!("cd: {path}: {}", err::name(-r));
            1
        }
        _ => 0,
    }
}

fn pwd(_: &mut AppHost, _: &[&str]) -> i32 {
    println!("{}", crate::host::cwd());
    0
}

fn sleep(_: &mut AppHost, args: &[&str]) -> i32 {
    let ms: u32 = match args.get(1).map(|s| s.parse()) {
        None => 500,
        Some(Ok(ms)) => ms,
        Some(Err(_)) => {
            eprintln!("usage: sleep [ms]");
            return 2;
        }
    };
    let start = sys::ticks();
    sys::sleep(ms);
    let elapsed = sys::ticks().wrapping_sub(start);
    let expected = (ms * TICK_HZ).div_ceil(1000);
    let ok = elapsed >= expected && elapsed <= expected + expected / 5 + 2;
    println!("slept {} ticks: {}", elapsed, if ok { "sleep ok" } else { "sleep NG" });
    0
}

fn history(sh: &mut AppHost, _: &[&str]) -> i32 {
    for (i, h) in sh.editor.history().enumerate() {
        println!("{:3} {h}", i + 1);
    }
    0
}

fn term(_: &mut AppHost, _: &[&str]) -> i32 {
    if sys::term_open("") < 0 {
        eprintln!("term: too many terminals");
        return 1;
    }
    0
}

/// デスクトップの背景の画像: BMP を読んで標準のパレットに直し、背景の大きさに並べてカーネルに渡す
fn wallpaper(_: &mut AppHost, args: &[&str]) -> i32 {
    use vibe_rt::gfx_lib::bmp::Bmp;
    use vibe_rt::gfx_lib::wallpaper;
    const USAGE: &str = "usage: wallpaper file.bmp [tile|center] | none";
    if args[1..] == ["none"] {
        sys::wallpaper_set(&[], 0, 0, 0);
        return 0;
    }
    let setting = args[1..].join(" ");
    let Some((path, fit)) = (args.len() > 1).then(|| wallpaper::parse_setting(&setting)).flatten() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let set = || -> Result<(), String> {
        sys::wallpaper_size().map_err(|_| String::from("only on the desktop"))?;
        let data = vibe_rt::read_file(path).map_err(|e| String::from(err::name(e)))?;
        let bmp = Bmp::parse(&data).map_err(|e| format!("{e}"))?;
        let pixels = bmp.to_rgb32();
        match sys::wallpaper_set(&pixels, bmp.width, bmp.height, wallpaper::fit_code(fit)) {
            0 => Ok(()),
            e => Err(String::from(err::name(-e))),
        }
    };
    match set() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("wallpaper: {path}: {e}");
            1
        }
    }
}
