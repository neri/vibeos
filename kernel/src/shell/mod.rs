//! 内蔵シェル（独立したカーネルスレッドで動く）
//!
//! 1 行読んで空白で区切り（"..." で空白を含められる）、コマンドの表から探して実行する。
//! コマンドを追加するときは `commands.rs` の `COMMANDS` に足す。

mod commands;
mod line;

use alloc::string::String;
use alloc::vec::Vec;

use line::{Input, LineEditor};

use crate::fs;

/// シェルの状態（コマンドから参照する）
pub struct Shell {
    editor: LineEditor,
}

pub fn run() -> ! {
    kprintln!("\x1b[93mVibeOS shell\x1b[0m - type 'help' for commands");
    let mut shell = Shell { editor: LineEditor::new() };
    loop {
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
            None => kprintln!("unknown command: {}", argv[0]),
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
