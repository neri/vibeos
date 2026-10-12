//! ps: タスク（カーネルスレッドとプロセス）の一覧

#![no_std]
#![no_main]

use vibe_rt::abi::{self, ProcInfo};
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let mut list = [ProcInfo::EMPTY; 64];
    let n = sys::proc_list(&mut list).max(0) as usize;
    println!("  ID NAME       STATE    TICKS");
    for p in &list[..n] {
        let state = match p.state {
            abi::PROC_READY => "Ready",
            abi::PROC_RUNNING => "Running",
            abi::PROC_BLOCKED => "Blocked",
            _ => continue, // 終わってまだ片付いていないもの
        };
        println!("{:4} {:10} {:8} {}", p.id, p.name(), state, p.ticks);
    }
    0
}
