//! 内蔵シェル（端末ごとに 1 つ、独立したカーネルスレッドで動く）
//!
//! 入出力は今のタスクの端末（`sched::current_tty`）。端末が閉じられたら終わる。
//! 1 行を `vibe_shell::parse` でコマンドの並びとつなぎ方（`|`・`<`・`>`・`>>`・`2>&1`・`&`）に分け、
//! コマンドの表にあれば内蔵コマンドとして、無ければ /BIN のプログラムとして実行する。
//! コマンドを追加するときは `commands.rs` の `COMMANDS` に足す。
//!
//! 内蔵コマンドの動かし方（docs/plans/pipe.md §3.6）:
//! - 1 つだけなら、シェルのスレッドで動かす（`cd` などが効くように）。リダイレクトがあれば、その間だけ
//!   シェルの fd 0〜2 をつなぎ替える
//! - パイプの中か `&` なら、fd 0〜2 をつないだ短い間だけのカーネルスレッドで動かす（同じスレッドで
//!   書く側と読む側を動かすと待ち合って止まるため）。シェルの状態を変えても、シェルには効かない

/// 内蔵コマンドの結果（Err は書けなかった・止めるよう頼まれた）
pub type Out = Result<(), i32>;

/// 内蔵コマンドの出力（fd 1）。書けなければ `?` で抜ける
macro_rules! out {
    ($($arg:tt)*) => { $crate::shell::write_fmt(1, format_args!($($arg)*))? };
}

macro_rules! outln {
    () => { out!("\n") };
    ($($arg:tt)*) => { $crate::shell::write_fmt(1, format_args!("{}\n", format_args!($($arg)*)))? };
}

/// 内蔵コマンドのエラー（fd 2）
macro_rules! errln {
    ($($arg:tt)*) => { $crate::shell::write_fmt(2, format_args!("{}\n", format_args!($($arg)*)))? };
}

mod commands;
pub mod line;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use line::{Input, LineEditor};
use vibe_shell::{Pipeline, Redirect, Target};

use crate::fs::{self, Desc};
use crate::io;
use crate::task::{self, sched};

/// 止めるよう頼まれていたら EINTR（内蔵コマンドは読み書きのたびに確かめる）
fn check_stop() -> Out {
    if crate::process::kill_pending(sched::current_id()) { Err(vibe_abi::err::EINTR) } else { Ok(()) }
}

pub fn write_fmt(fd: fs::Fd, args: core::fmt::Arguments) -> Out {
    check_stop()?;
    let text = alloc::fmt::format(args);
    io::write_all(fd, text.as_bytes())
}

/// fd 1 にバイト列を書く
pub fn out_bytes(data: &[u8]) -> Out {
    check_stop()?;
    io::write_all(1, data)
}

/// fd 1 が端末か（色や表示できない文字の置き換えを端末のときだけにする）
pub fn stdout_is_console() -> bool {
    fs::get(1).is_ok_and(|d| matches!(*d, Desc::Console))
}

/// シェルの状態（コマンドから参照する）
#[derive(Clone)]
pub struct Shell {
    editor: LineEditor,
}

/// シェルを動かす。端末が閉じられたら戻る。`command` があれば、それだけを実行して戻る
pub fn run(command: Option<String>) {
    let mut shell = Shell { editor: LineEditor::new() };
    if let Some(line) = command {
        execute(&mut shell, &line);
        return;
    }
    kprintln!("\x1b[93mVibeOS shell\x1b[0m - type 'help' for commands");
    let tty = crate::task::sched::current_tty();
    while !crate::gui::is_closing(tty) {
        let prompt = alloc::format!("\x1b[92m{}\x1b[0m> ", fs::cwd());
        let line = match shell.editor.read_line(&prompt) {
            Input::Line(l) => l,
            Input::Cancelled => continue,
        };
        execute(&mut shell, &line);
    }
}

/// 1 行のコマンドを実行する
fn execute(shell: &mut Shell, line: &str) {
    let p = match vibe_shell::parse(line) {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            kprintln!("syntax error: {e}");
            return;
        }
    };
    let first = &p.commands[0];
    if p.commands.len() == 1 && !p.background {
        if let Some(cmd) = commands::find(&first.argv[0]) {
            let argv: Vec<&str> = first.argv.iter().map(String::as_str).collect();
            if first.redirects.is_empty() {
                let _ = (cmd.run)(shell, &argv);
                return;
            }
            // リダイレクトの間だけシェルの fd 0〜2 をつなぎ替える
            let mut stdio = fs::stdio();
            if let Err(msg) = apply_redirects(&mut stdio, &first.redirects) {
                kprintln!("{msg}");
                return;
            }
            let saved = fs::swap_stdio(stdio);
            let _ = (cmd.run)(shell, &argv);
            drop(fs::swap_stdio(saved));
            return;
        }
    }
    run_pipeline(shell, &p);
}

/// リダイレクトを左から順に行う
fn apply_redirects(stdio: &mut fs::Stdio, redirects: &[Redirect]) -> Result<(), String> {
    for r in redirects {
        let fd = r.fd as usize;
        let (path, flags) = match &r.target {
            Target::Dup(m) => {
                stdio[fd] = stdio[*m as usize].clone();
                continue;
            }
            Target::Read(p) => (p, 0),
            Target::Write(p) => (p, fs::OPEN_WRITE | fs::OPEN_CREATE | fs::OPEN_TRUNCATE),
            Target::Append(p) => (p, fs::OPEN_WRITE | fs::OPEN_CREATE | fs::OPEN_APPEND),
        };
        stdio[fd] = Some(fs::open_desc(path, flags).map_err(|e| alloc::format!("{path}: {e}"))?);
    }
    Ok(())
}

/// 起動したもの
enum Started {
    Process(task::TaskId),
    /// 内蔵コマンドを動かしているカーネルスレッド（終了コードは無い）
    Thread(task::TaskId),
}

/// コマンドを（内蔵コマンドはカーネルスレッドで、ほかは /BIN のプログラムとして）パイプでつないで
/// 起動し、全部が終わるのを待つ（`&` なら待たない）。終了コードは最後のコマンドのものを見る
fn run_pipeline(shell: &Shell, p: &Pipeline) {
    use crate::process;
    if p.commands.len() > crate::gui::MAX_FOREGROUND {
        kprintln!("too many commands in a pipeline");
        return;
    }
    let shell_stdio = fs::stdio();
    let mut started = Vec::new();
    let mut next_stdin = shell_stdio[0].clone();
    for (i, cmd) in p.commands.iter().enumerate() {
        // この後ろとのパイプ
        let (stdout, following) = if i + 1 < p.commands.len() {
            let (r, w) = crate::pipe::new();
            (Some(Arc::new(Desc::PipeWrite(w))), Some(Arc::new(Desc::PipeRead(r))))
        } else {
            (shell_stdio[1].clone(), None)
        };
        let mut stdio = [core::mem::replace(&mut next_stdin, following), stdout, shell_stdio[2].clone()];
        let name = cmd.argv[0].as_str();
        if let Err(msg) = apply_redirects(&mut stdio, &cmd.redirects) {
            kprintln!("{msg}");
            continue;
        }
        let last = i + 1 == p.commands.len();
        if let Some(c) = commands::find(name) {
            let argv = cmd.argv.clone();
            let mut sh = shell.clone();
            let id = sched::spawn_with_stdio(name, stdio, move || {
                let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                let _ = (c.run)(&mut sh, &argv);
            });
            started.push((Started::Thread(id), name, last));
            continue;
        }
        // argv[0] は打った名前のまま渡す
        let argv: Vec<&str> = cmd.argv.iter().map(String::as_str).collect();
        let path = process::resolve_program(name);
        match process::spawn_ex(&path, &argv, &[], Some(stdio)) {
            Ok(id) => started.push((Started::Process(id), name, last)),
            Err(vibe_abi::err::ENOENT) => kprintln!("unknown command: {name}"),
            Err(e) => kprintln!("{name}: {}", vibe_abi::err::name(e)),
        }
    }
    // シェルが持っているパイプの端はここで手放す（でないと読む側に終わりが来ない）
    drop(next_stdin);
    let id_of = |s: &Started| match *s {
        Started::Process(id) | Started::Thread(id) => id,
    };
    if p.background {
        for (s, name, _) in &started {
            if let Started::Process(id) = s {
                process::detach(*id);
            }
            kprintln!("[{}] {name}", id_of(s));
        }
        return;
    }
    // 前面のプロセスにする（Ctrl+C で全部を止められる）
    let ids: Vec<_> = started.iter().map(|s| id_of(&s.0)).collect();
    process::set_foreground(&ids);
    let results: Vec<_> = started
        .iter()
        .map(|(s, name, last)| {
            let r = match *s {
                Started::Process(id) => process::wait(id),
                Started::Thread(id) => {
                    task::join(id);
                    Ok(0)
                }
            };
            (*name, *last, r)
        })
        .collect();
    process::set_foreground(&[]);
    if let Some((name, true, result)) = results.last() {
        match result {
            Ok(0) => {}
            Ok(code) => kprintln!("[{name} exited with {code}]"),
            Err(e) => kprintln!("{name}: {}", vibe_abi::err::name(*e)),
        }
    }
}
