//! sh: ユーザ空間のシェル（docs/plans/user-shell.md）
//!
//! sh                    端末で対話する（標準入力が端末でなければ、標準入力をスクリプトとして読む）
//! sh -l                 対話する前に /AUTORUN.SH があれば実行する（起動したときの最初の端末）
//! sh ファイル [引数...]  スクリプトを実行する（$0 はファイル、$1... は引数）
//! sh -i ファイル [引数...]  スクリプトを実行してから対話する（files で .SH を開いたとき）
//! sh -c 文字列 [名前 [引数...]]  文字列を実行する（$0 は名前）
//!
//! 文法と実行は `vibe_shell::interp`。プロセスの起動などは `host.rs`、アプリの内蔵コマンドは
//! `builtins.rs`。見た目（プロンプト・メッセージ）は前のカーネルの中のシェルと同じ。

#![no_std]
#![no_main]

extern crate alloc;

mod builtins;
mod host;
mod term;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_line::Input;
use vibe_rt::{abi, eprintln, println};
use vibe_shell::interp::{Shell, is_incomplete};

use host::AppHost;

vibe_rt::entry!(main);

/// 起動したときに最初の端末が実行するスクリプト
const AUTORUN: &str = "/AUTORUN.SH";

fn main() -> i32 {
    let args: Vec<String> = vibe_rt::args().map(String::from).collect();
    let mode = args.get(1).map(String::as_str);
    let interactive = matches!(mode, None | Some("-l" | "-i")) && vibe_rt::isatty(abi::STDIN);
    let mut sh = Shell::new(AppHost::new(interactive));
    // 受け継いだ環境変数は export したものとして持つ
    for (k, v) in vibe_rt::env::vars() {
        sh.export(&k, Some(&v));
    }
    match args.get(1).map(String::as_str) {
        Some("-c") => {
            let Some(src) = args.get(2) else {
                eprintln!("sh: -c needs a command");
                return 2;
            };
            if let Some(name) = args.get(3) {
                sh.arg0 = name.clone();
            }
            sh.args = args.iter().skip(4).cloned().collect();
            sh.run(src);
        }
        Some("-l") if interactive => {
            let script = vibe_rt::read_file(AUTORUN).ok();
            return interact(&mut sh, script.map(|d| (String::from(AUTORUN), d)));
        }
        Some("-i") if interactive => {
            let Some(path) = args.get(2) else {
                eprintln!("sh: -i needs a file");
                return 2;
            };
            let data = match vibe_rt::read_file(path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("sh: {path}: {}", abi::err::name(e));
                    Vec::new()
                }
            };
            sh.args = args.iter().skip(3).cloned().collect();
            return interact(&mut sh, Some((path.clone(), data)));
        }
        Some(path) => {
            let src = match vibe_rt::read_file(path) {
                Ok(d) => String::from_utf8_lossy(&d).into_owned(),
                Err(e) => {
                    eprintln!("sh: {path}: {}", abi::err::name(e));
                    return NOT_FOUND;
                }
            };
            sh.arg0 = String::from(path);
            sh.args = args.iter().skip(2).cloned().collect();
            sh.run(&src);
        }
        None if interactive => return interact(&mut sh, None),
        None => match vibe_rt::io::read_to_end(abi::STDIN) {
            Ok(d) => sh.run(&String::from_utf8_lossy(&d)),
            Err(e) => {
                eprintln!("sh: {}", abi::err::name(e));
                return 1;
            }
        },
    }
    sh.exit.unwrap_or(sh.status)
}

const NOT_FOUND: i32 = vibe_shell::interp::NOT_FOUND;

/// 端末で対話する。`script`（名前, 中身）があれば、先にこのシェルの中で実行する
fn interact(sh: &mut Shell<AppHost>, script: Option<(String, Vec<u8>)>) -> i32 {
    println!("\x1b[93mVibeOS shell\x1b[0m - type 'help' for commands");
    sh.interactive = true;
    // Ctrl+C で自分は止まらず、実行中のコマンドをやめる
    vibe_rt::net::catch_ctrl_c();
    if let Some((name, data)) = script {
        let saved = core::mem::replace(&mut sh.arg0, name);
        sh.host.set_running(true);
        sh.run(&String::from_utf8_lossy(&data));
        sh.arg0 = saved;
        if let Some(code) = sh.exit {
            return code;
        }
    }
    let mut term = term::Terminal { closed: false };
    loop {
        sh.host.set_running(false);
        let prompt = format!("\x1b[92m{}\x1b[0m> ", host::cwd());
        let mut src = match sh.host.editor.read_line(&mut term, &prompt) {
            Input::Line(l) => l,
            // キーがもう読めない（端末が閉じた）なら終わる
            Input::Cancelled if term.closed => return sh.status,
            Input::Cancelled => continue,
        };
        // if や引用符が閉じていなければ、続きの行を読む
        while is_incomplete(&src) {
            match sh.host.editor.read_line(&mut term, "> ") {
                Input::Line(l) => {
                    src.push('\n');
                    src.push_str(&l);
                }
                Input::Cancelled => {
                    src.clear();
                    break;
                }
            }
        }
        if term.closed {
            return sh.status;
        }
        // 前に押された Ctrl+C の印は捨てる
        let _ = vibe_rt::net::ctrl_c_pressed();
        sh.host.set_running(true);
        sh.run(&src);
        if let Some(code) = sh.exit {
            return code;
        }
    }
}
