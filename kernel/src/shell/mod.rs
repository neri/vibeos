//! 端末のシェルの起動と、予備のシェル
//!
//! 端末のシェルはユーザ空間の `sh`（/BIN/SH.ELF）。端末のスレッドはそれを起動して終わるのを待つ
//! （`run_terminal`）。SH.ELF が無い・起動できないときだけ、このカーネルの中の予備のシェル（`run`）が
//! 動く。予備のシェルはプログラムを起動するだけ（内蔵コマンドは無い。`|`・`<`・`>`・`&` は使える）。

pub mod line;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use line::{Input, LineEditor};
use vibe_shell::{Pipeline, Redirect, Target};

use crate::fs::{self, Desc};
use crate::task::sched;

/// 端末のシェル（ユーザ空間の sh）
const SH_PATH: &str = "/BIN/SH.ELF";

/// 端末のシェルを動かす（端末のスレッドから呼ぶ）: /BIN/SH.ELF を起動して終わるのを待つ。
/// `command` があれば `sh -c command`。起動できなければ予備のシェルを動かす。
/// 最初の端末は閉じないので、sh が終わったら起動し直す。最初の端末で最初に起動する sh は `sh -l`
/// （/AUTORUN.SH があれば実行してから対話する）
pub fn run_terminal(command: Option<String>) {
    let tty = sched::current_tty();
    let mut first = tty == crate::gui::CONSOLE;
    loop {
        let mut argv = alloc::vec!["sh"];
        if let Some(c) = &command {
            argv.extend(["-c", c.as_str()]);
        } else if core::mem::take(&mut first) {
            argv.push("-l");
        }
        let id = match crate::process::spawn_ex(SH_PATH, &argv, &[], None) {
            Ok(id) => id,
            Err(e) => {
                kprintln!("cannot start {SH_PATH} ({}), using the built-in shell", vibe_abi::err::name(e));
                run(command);
                return;
            }
        };
        let _ = crate::process::wait(id);
        if command.is_some() || tty != crate::gui::CONSOLE || crate::gui::is_closing(tty) {
            return;
        }
        klog!("term: shell exited, starting it again");
        crate::drivers::pit::sleep_ms(500);
    }
}

/// 予備のシェルを動かす。端末が閉じられたら戻る。`command` があれば、それだけを実行して戻る
pub fn run(command: Option<String>) {
    if let Some(line) = command {
        execute(&line);
        return;
    }
    kprintln!("\x1b[93mVibeOS shell\x1b[0m (built-in, programs only)");
    let mut editor = LineEditor::new();
    let tty = sched::current_tty();
    while !crate::gui::is_closing(tty) {
        let prompt = alloc::format!("\x1b[92m{}\x1b[0m> ", fs::cwd());
        match editor.read_line(&prompt) {
            Input::Line(l) => execute(&l),
            Input::Cancelled => {}
        }
    }
}

/// 1 行を実行する
fn execute(line: &str) {
    match vibe_shell::parse(line) {
        Ok(Some(p)) => run_pipeline(&p),
        Ok(None) => {}
        Err(e) => kprintln!("syntax error: {e}"),
    }
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

/// /BIN のプログラムをパイプでつないで起動し、全部が終わるのを待つ（`&` なら待たない）
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
