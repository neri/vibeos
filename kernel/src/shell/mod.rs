//! 内蔵シェル（端末ごとに 1 つ、独立したカーネルスレッドで動く）
//!
//! 入出力は今のタスクの端末（`sched::current_tty`）。端末が閉じられたら終わる。
//! 1 行を `vibe_shell::parse` でコマンドの並びとつなぎ方（`|`・`<`・`>`・`>>`・`2>&1`・`&`）に分け、
//! コマンドの表にあれば内蔵コマンドとして、無ければ /BIN のプログラムとして実行する。
//! コマンドを追加するときは `commands.rs` の `COMMANDS` に足す。

mod commands;
pub mod line;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use line::{Input, LineEditor};
use vibe_shell::{Pipeline, Redirect, Target};

use crate::fs::{self, Desc};

/// シェルの状態（コマンドから参照する）
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
    if p.commands.len() == 1 && first.redirects.is_empty() && !p.background {
        if let Some(cmd) = commands::find(&first.argv[0]) {
            let argv: Vec<&str> = first.argv.iter().map(String::as_str).collect();
            (cmd.run)(shell, &argv);
            return;
        }
    }
    if let Some(c) = p.commands.iter().find(|c| commands::find(&c.argv[0]).is_some()) {
        kprintln!("{}: built-in commands cannot be used with |, <, > or &", c.argv[0]);
        return;
    }
    run_pipeline(&p);
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

/// /BIN の中のプログラムを、パイプでつないで起動し、全部が終わるのを待つ（`&` なら待たない）。
/// 終了コードは最後のコマンドのものを見る
fn run_pipeline(p: &Pipeline) {
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
        // argv[0] は打った名前のまま渡す
        let argv: Vec<&str> = cmd.argv.iter().map(String::as_str).collect();
        let path = process::resolve_program(name);
        match process::spawn_ex(&path, &argv, &[], Some(stdio)) {
            Ok(id) => started.push((id, name, i + 1 == p.commands.len())),
            Err(vibe_abi::err::ENOENT) => kprintln!("unknown command: {name}"),
            Err(e) => kprintln!("{name}: {}", vibe_abi::err::name(e)),
        }
    }
    // シェルが持っているパイプの端はここで手放す（でないと読む側に終わりが来ない）
    drop(next_stdin);
    if p.background {
        for (id, name, _) in started {
            process::detach(id);
            kprintln!("[{id}] {name}");
        }
        return;
    }
    // 前面のプロセスにする（Ctrl+C で全部を止められる）
    let ids: Vec<_> = started.iter().map(|s| s.0).collect();
    process::set_foreground(&ids);
    let results: Vec<_> = started.iter().map(|&(id, name, last)| (name, last, process::wait(id))).collect();
    process::set_foreground(&[]);
    if let Some((name, true, result)) = results.last() {
        match result {
            Ok(0) => {}
            Ok(code) => kprintln!("[{name} exited with {code}]"),
            Err(e) => kprintln!("{name}: {}", vibe_abi::err::name(*e)),
        }
    }
}
