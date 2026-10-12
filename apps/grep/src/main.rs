//! grep [-i] [-v] [-n] [-c] 文字列 [ファイル...]: 文字列を含む行を出す（無ければ標準入力）
//!
//! -i 大文字と小文字を区別しない, -v 含まない行, -n 行番号を付ける, -c 行の数だけ。
//! 終了コードは、出す行があれば 0、無ければ 1、誤りなら 2

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::{eprintln, println};
use vibe_text::{Grep, parse_options};

vibe_rt::entry!(main);

fn main() -> i32 {
    let Ok(o) = parse_options(vibe_rt::args().skip(1), "ivnc", "") else {
        return usage();
    };
    let Some((pattern, files)) = o.rest.split_first() else {
        return usage();
    };
    let g = Grep { pattern, ignore_case: o.has('i'), invert: o.has('v') };
    let show_name = files.len() > 1;
    let mut found = false;
    let ok = vibe_rt::io::for_each_input("grep", files, |name, data| {
        let text = String::from_utf8_lossy(data);
        let mut count = 0;
        for (i, line) in vibe_text::lines(&text).enumerate() {
            if !g.matches(line) {
                continue;
            }
            found = true;
            count += 1;
            if o.has('c') {
                continue;
            }
            let mut prefix = Vec::new();
            if show_name {
                prefix.push(String::from(name));
            }
            if o.has('n') {
                prefix.push(alloc::format!("{}", i + 1));
            }
            if prefix.is_empty() {
                println!("{line}");
            } else {
                println!("{}:{line}", prefix.join(":"));
            }
        }
        if o.has('c') {
            if show_name {
                println!("{name}:{count}");
            } else {
                println!("{count}");
            }
        }
    });
    if !ok {
        2
    } else if found {
        0
    } else {
        1
    }
}

fn usage() -> i32 {
    eprintln!("usage: grep [-i] [-v] [-n] [-c] text [file...]");
    2
}
