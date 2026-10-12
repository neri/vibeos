//! sort [-r] [-n] [-u] [ファイル...]: 行を並べ替える（無ければ標準入力）
//!
//! -r 逆の順, -n 先頭の数で比べる, -u 同じ行は 1 つ

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::{eprintln, println};
use vibe_text::{Sort, parse_options};

vibe_rt::entry!(main);

fn main() -> i32 {
    let Ok(o) = parse_options(vibe_rt::args().skip(1), "rnu", "") else {
        eprintln!("usage: sort [-r] [-n] [-u] [file...]");
        return 2;
    };
    let mut text = String::new();
    let ok = vibe_rt::io::for_each_input("sort", &o.rest, |_, data| {
        text.push_str(&String::from_utf8_lossy(data));
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
    });
    let s = Sort { reverse: o.has('r'), numeric: o.has('n'), unique: o.has('u') };
    let lines: Vec<&str> = vibe_text::lines(&text).collect();
    for line in s.sort(lines) {
        println!("{line}");
    }
    if ok { 0 } else { 1 }
}
