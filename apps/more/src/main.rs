//! テキストを 1 画面ずつ表示する
//!
//! more [ファイル]。ファイルが無ければ標準入力を読む（`ls | more`）。キーは端末から読む。
//! スペース = 次の画面、Enter = 1 行進む、q / Esc = 終わり。出力が端末でなければ止めずに全部書く。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::{abi, eprintln, print, println, sys};

vibe_rt::entry!(main);

/// 端末の大きさが分からないときの大きさ（320x200 の端末）
const DEFAULT_SIZE: (usize, usize) = (40, 24);

fn main() -> i32 {
    let data = match vibe_rt::args().nth(1) {
        Some(path) => vibe_rt::read_file(path).map_err(|e| (path, e)),
        None if !vibe_rt::isatty(abi::STDIN) => vibe_rt::io::read_to_end(abi::STDIN).map_err(|e| ("-", e)),
        None => {
            eprintln!("usage: more [file]");
            return 2;
        }
    };
    let data = match data {
        Ok(d) => d,
        Err((path, e)) => {
            eprintln!("more: {path}: {}", abi::err::name(e));
            return 1;
        }
    };
    if !vibe_rt::isatty(abi::STDOUT) {
        return if vibe_rt::io::write_all(abi::STDOUT, &data).is_ok() { 0 } else { 1 };
    }
    // 1 画面の行数（1 行はプロンプト用に残す）
    let (cols, rows) = sys::term_size().unwrap_or(DEFAULT_SIZE);
    let page = rows - 1;
    let text: String = data.iter().map(|&b| if b == b'\n' || (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
    // 画面の幅で折り返した行に分ける
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        }
        for chunk in chars.chunks(cols) {
            lines.push(chunk.iter().collect());
        }
    }

    let mut shown = 0;
    let mut want = page;
    while shown < lines.len() {
        while shown < want.min(lines.len()) {
            // ちょうど端末の幅の行は自動で折り返されるので改行を足さない
            if lines[shown].len() == cols {
                print!("{}", lines[shown]);
            } else {
                println!("{}", lines[shown]);
            }
            shown += 1;
        }
        if shown >= lines.len() {
            break;
        }
        print!("\x1b[7m\x1b[30;47m-- more ({}%) --\x1b[0m", shown * 100 / lines.len());
        let k = loop {
            if let Some(k) = sys::getkey(false) {
                if k.pressed() {
                    break k;
                }
            }
        };
        print!("\r\x1b[K");
        match (k.key(), k.ch()) {
            (abi::key::ESCAPE, _) | (_, Some('q')) => return 0,
            (abi::key::ENTER, _) => want = shown + 1,
            _ => want = shown + page,
        }
    }
    0
}
