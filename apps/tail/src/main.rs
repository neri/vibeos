//! tail [-n 行数 | -行数] [ファイル...]: 末尾の行（既定 10 行。無ければ標準入力）

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;

use vibe_rt::{eprintln, println};
use vibe_text::{line_count, parse_options};

vibe_rt::entry!(main);

fn main() -> i32 {
    let o = parse_options(vibe_rt::args().skip(1), "n#", "n");
    let Some((o, n)) = o.ok().and_then(|o| line_count(&o).ok().map(|n| (o, n))) else {
        eprintln!("usage: tail [-n lines] [file...]");
        return 2;
    };
    let many = o.rest.len() > 1;
    let ok = vibe_rt::io::for_each_input("tail", &o.rest, |name, data| {
        if many {
            println!("==> {name} <==");
        }
        for line in vibe_text::tail(&String::from_utf8_lossy(data), n) {
            println!("{line}");
        }
    });
    if ok { 0 } else { 1 }
}
