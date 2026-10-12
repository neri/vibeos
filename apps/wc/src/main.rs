//! wc [-l] [-w] [-c] [ファイル...]: 行・語・バイトの数（無ければ標準入力）
//!
//! オプションを 1 つだけ付けて標準入力を読んだときは、数だけを出す（`ls | wc -l`）

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::{eprintln, println};
use vibe_text::{Counts, parse_options};

vibe_rt::entry!(main);

fn main() -> i32 {
    let Ok(o) = parse_options(vibe_rt::args().skip(1), "lwc", "") else {
        eprintln!("usage: wc [-l] [-w] [-c] [file...]");
        return 2;
    };
    let all = !(o.has('l') || o.has('w') || o.has('c'));
    let show = |c: &Counts, name: Option<&str>| {
        let mut cols: Vec<String> = Vec::new();
        for (flag, v) in [('l', c.lines), ('w', c.words), ('c', c.bytes)] {
            if all || o.has(flag) {
                cols.push(alloc::format!("{v}"));
            }
        }
        match name {
            Some(n) => println!("{} {n}", cols.join(" ")),
            None => println!("{}", cols.join(" ")),
        }
    };
    let mut total = Counts::default();
    let mut count = 0;
    let ok = vibe_rt::io::for_each_input("wc", &o.rest, |name, data| {
        let c = Counts::of(data);
        total.lines += c.lines;
        total.words += c.words;
        total.bytes += c.bytes;
        count += 1;
        show(&c, (!o.rest.is_empty()).then_some(name));
    });
    if count > 1 {
        show(&total, Some("total"));
    }
    if ok { 0 } else { 1 }
}
