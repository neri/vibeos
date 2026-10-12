//! 内蔵コマンド（このプロセスの中で動く。終了コードを返す）
//!
//! 出力は標準出力、エラーは標準エラー。カーネルの中でしかできないもの（`hwclock`・`wallpaper` など）は
//! 今はカーネルの内蔵コマンドを `kcmd` で呼ぶ（S3 でアプリとシステムコールにする）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::abi::{self, DirEntry, OPEN_APPEND, OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE, TICK_HZ, err};
use vibe_rt::{eprintln, io, print, println, sys};

use crate::Shell;

pub struct Builtin {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    pub run: fn(&mut Shell, &[&str]) -> i32,
}

const BUILTINS: &[Builtin] = &[
    Builtin { name: "help", usage: "help [command]", help: "show commands", run: help },
    Builtin { name: "clear", usage: "clear", help: "clear the screen", run: clear },
    Builtin { name: "echo", usage: "echo [text...]", help: "print text", run: echo },
    Builtin { name: "ls", usage: "ls [dir]", help: "list a directory", run: ls },
    Builtin { name: "cat", usage: "cat [file...]", help: "print files (or standard input)", run: cat },
    Builtin { name: "write", usage: "write file text...", help: "write text to a file (replaces it)", run: write },
    Builtin { name: "append", usage: "append file text...", help: "add a line of text to a file", run: append },
    Builtin { name: "cp", usage: "cp from to", help: "copy a file", run: cp },
    Builtin { name: "rm", usage: "rm path", help: "remove a file or an empty directory", run: rm },
    Builtin { name: "mv", usage: "mv from to", help: "rename or move a file or directory", run: mv },
    Builtin { name: "mkdir", usage: "mkdir dir", help: "make a directory", run: mkdir },
    Builtin { name: "sync", usage: "sync", help: "save changes to the floppy disk", run: sync },
    Builtin { name: "cd", usage: "cd [dir]", help: "change directory", run: cd },
    Builtin { name: "pwd", usage: "pwd", help: "print current directory", run: pwd },
    Builtin { name: "hwclock", usage: "hwclock", help: "read the CMOS clock", run: kernel },
    Builtin { name: "sleep", usage: "sleep [ms]", help: "sleep (default 500ms, checks timing)", run: sleep },
    Builtin { name: "history", usage: "history", help: "command history", run: history },
    Builtin { name: "term", usage: "term", help: "open a new terminal (Alt+F1-F4 to switch)", run: term },
    Builtin {
        name: "wallpaper",
        usage: "wallpaper file.bmp [tile|center] | none",
        help: "set the desktop background image",
        run: kernel,
    },
    Builtin { name: "demo", usage: "demo", help: "3 threads draw counters for 5s", run: kernel },
    Builtin { name: "beep", usage: "beep [hz] [ms]", help: "sound the PC speaker", run: kernel },
    Builtin { name: "reboot", usage: "reboot", help: "restart the computer", run: kernel },
    Builtin { name: "exit", usage: "exit [code]", help: "end the shell", run: exit },
    Builtin { name: "divzero", usage: "divzero", help: "test: divide by zero", run: kernel },
    Builtin { name: "nullptr", usage: "nullptr", help: "test: read address 0", run: kernel },
    Builtin { name: "overflow", usage: "overflow", help: "test: kernel stack overflow", run: kernel },
    Builtin { name: "panic", usage: "panic", help: "test: kernel panic", run: kernel },
];

pub fn find(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

fn help(_: &mut Shell, args: &[&str]) -> i32 {
    if let Some(name) = args.get(1) {
        return match find(name) {
            Some(b) => {
                println!("{}\n  {}", b.usage, b.help);
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
    for b in BUILTINS {
        if line.len() + b.name.len() + 1 > width - 2 {
            println!("{line}");
            line.clear();
        }
        line.push_str(b.name);
        line.push(' ');
    }
    println!("{line}");
    println!("'help <command>' for details. Other commands are programs in /BIN");
    0
}

fn clear(_: &mut Shell, _: &[&str]) -> i32 {
    print!("\x1b[2J\x1b[H");
    0
}

fn echo(_: &mut Shell, args: &[&str]) -> i32 {
    println!("{}", args[1..].join(" "));
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

fn ls(_: &mut Shell, args: &[&str]) -> i32 {
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

fn cat(_: &mut Shell, args: &[&str]) -> i32 {
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

fn write_text(args: &[&str], flags: u32) -> i32 {
    let name = args[0];
    let Some(path) = args.get(1) else {
        eprintln!("usage: {name} file text...");
        return 2;
    };
    let mut text = args[2..].join(" ");
    text.push('\n');
    match write_to(path, text.as_bytes(), flags) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{name}: {path}: {}", err::name(e));
            1
        }
    }
}

fn write(_: &mut Shell, args: &[&str]) -> i32 {
    write_text(args, OPEN_TRUNCATE)
}

fn append(_: &mut Shell, args: &[&str]) -> i32 {
    write_text(args, OPEN_APPEND)
}

fn cp(_: &mut Shell, args: &[&str]) -> i32 {
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

fn rm(_: &mut Shell, args: &[&str]) -> i32 {
    path_op(args, "rm path", sys::remove)
}

fn mkdir(_: &mut Shell, args: &[&str]) -> i32 {
    path_op(args, "mkdir dir", sys::mkdir)
}

fn mv(_: &mut Shell, args: &[&str]) -> i32 {
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

fn sync(_: &mut Shell, _: &[&str]) -> i32 {
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

fn cd(_: &mut Shell, args: &[&str]) -> i32 {
    let path = args.get(1).copied().unwrap_or("/");
    match sys::chdir(path) {
        r if r < 0 => {
            eprintln!("cd: {path}: {}", err::name(-r));
            1
        }
        _ => 0,
    }
}

fn pwd(_: &mut Shell, _: &[&str]) -> i32 {
    println!("{}", crate::cwd());
    0
}

fn sleep(_: &mut Shell, args: &[&str]) -> i32 {
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

fn history(sh: &mut Shell, _: &[&str]) -> i32 {
    for (i, h) in sh.editor.history().enumerate() {
        println!("{:3} {h}", i + 1);
    }
    0
}

fn term(_: &mut Shell, _: &[&str]) -> i32 {
    if sys::term_open("") < 0 {
        eprintln!("term: too many terminals");
        return 1;
    }
    0
}

fn exit(sh: &mut Shell, args: &[&str]) -> i32 {
    let code = match args.get(1).map(|s| s.parse::<i32>()) {
        None => sh.status,
        Some(Ok(c)) => c,
        Some(Err(_)) => {
            eprintln!("usage: exit [code]");
            return 2;
        }
    };
    sh.exit = Some(code);
    code
}

/// カーネルの中でしかできないコマンド（S3 まで）
fn kernel(_: &mut Shell, args: &[&str]) -> i32 {
    match sys::kcmd(args) {
        r if r < 0 => {
            eprintln!("{}: {}", args[0], err::name(-r));
            1
        }
        _ => 0,
    }
}
