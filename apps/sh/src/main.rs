//! sh: ユーザ空間のシェル（docs/plans/user-shell.md）
//!
//! sh            端末で対話する（プロンプト・行の編集・履歴）
//! sh -c 行      その 1 行を実行して終わる（端末の term_open、パイプの中の内蔵コマンド）
//!
//! 1 行を `vibe_shell::parse` でコマンドの並びとつなぎ方に分け、内蔵コマンドはこのプロセスの中で、
//! ほかは /BIN のプログラムとして実行する（`exec.rs`）。見た目（プロンプト・メッセージ）は前の
//! カーネルの中のシェルと同じ。

#![no_std]
#![no_main]

extern crate alloc;

mod builtins;
mod exec;
mod term;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_line::{Input, LineEditor};
use vibe_rt::{eprintln, println, sys};

vibe_rt::entry!(main);

/// シェルの状態
pub struct Shell {
    pub editor: LineEditor,
    /// 最後のコマンドの終了コード
    pub status: i32,
    /// exit が呼ばれた（終了コード）
    pub exit: Option<i32>,
}

fn main() -> i32 {
    let args: Vec<&str> = vibe_rt::args().collect();
    let mut sh = Shell { editor: LineEditor::new(), status: 0, exit: None };
    match args.get(1).copied() {
        Some("-c") => {
            let Some(line) = args.get(2) else {
                eprintln!("sh: -c needs a command");
                return 2;
            };
            exec::execute(&mut sh, line);
            sh.exit.unwrap_or(sh.status)
        }
        Some(other) => {
            eprintln!("usage: sh [-c command] (got {other})");
            2
        }
        None => interactive(&mut sh),
    }
}

fn interactive(sh: &mut Shell) -> i32 {
    println!("\x1b[93mVibeOS shell\x1b[0m - type 'help' for commands");
    let mut term = term::Terminal { closed: false };
    loop {
        let prompt = format!("\x1b[92m{}\x1b[0m> ", cwd());
        let line = match sh.editor.read_line(&mut term, &prompt) {
            Input::Line(l) => l,
            // キーがもう読めない（端末が閉じた）なら終わる
            Input::Cancelled if term.closed => return sh.status,
            Input::Cancelled => continue,
        };
        exec::execute(sh, &line);
        if let Some(code) = sh.exit {
            return code;
        }
    }
}

/// カレントディレクトリ
pub fn cwd() -> String {
    sys::getcwd().unwrap_or_else(|_| String::from("?"))
}
