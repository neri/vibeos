//! 内蔵シェル（端末ごとに 1 つ、独立したカーネルスレッドで動く）
//!
//! 入出力は今のタスクの端末（`sched::current_tty`）。端末が閉じられたら終わる。
//! 1 行読んで空白で区切り（"..." で空白を含められる）、コマンドの表から探して実行する。
//! コマンドを追加するときは `commands.rs` の `COMMANDS` に足す。

mod commands;
pub mod line;

use alloc::string::String;
use alloc::vec::Vec;

use line::{Input, LineEditor};

use crate::fs;

/// シェルの状態（コマンドから参照する）
pub struct Shell {
    editor: LineEditor,
}

/// シェルを動かす。端末が閉じられたら戻る
pub fn run() {
    kprintln!("\x1b[93mVibeOS shell\x1b[0m - type 'help' for commands");
    let tty = crate::task::sched::current_tty();
    let mut shell = Shell { editor: LineEditor::new() };
    while !crate::gui::is_closing(tty) {
        let prompt = alloc::format!("\x1b[92m{}\x1b[0m> ", fs::cwd());
        let line = match shell.editor.read_line(&prompt) {
            Input::Line(l) => l,
            Input::Cancelled => continue,
        };
        let args = split_args(&line);
        if args.is_empty() {
            continue;
        }
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        match commands::find(argv[0]) {
            Some(cmd) => (cmd.run)(&mut shell, &argv),
            None => run_program(&argv),
        }
    }
}

/// 空白で区切る。"..." の中の空白は区切らない
fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has_token = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has_token {
                    out.push(core::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(cur);
    }
    out
}

/// 組み込みのコマンドでなければ、/BIN の中のプログラムとして実行して終わるのを待つ。
/// 最後が "&" なら待たない（裏で動かす）
fn run_program(argv: &[&str]) {
    let (argv, background) = match argv.split_last() {
        Some((&"&", rest)) if !rest.is_empty() => (rest, true),
        _ => (argv, false),
    };
    // argv[0] は打った名前のまま渡す
    let path = crate::process::resolve_program(argv[0]);
    match crate::process::spawn(&path, argv) {
        Ok(id) if background => {
            crate::process::detach(id);
            kprintln!("[{}] {}", id, argv[0]);
        }
        Ok(id) => {
            // 前面のプロセスにする（Ctrl+C で止められる）
            crate::process::set_foreground(Some(id));
            let result = crate::process::wait(id);
            crate::process::set_foreground(None);
            match result {
                Ok(0) => {}
                Ok(code) => kprintln!("[{} exited with {}]", argv[0], code),
                Err(e) => kprintln!("{}: {}", argv[0], vibe_abi::err::name(e)),
            }
        }
        Err(vibe_abi::err::ENOENT) => kprintln!("unknown command: {}", argv[0]),
        Err(e) => kprintln!("{}: {}", argv[0], vibe_abi::err::name(e)),
    }
}
