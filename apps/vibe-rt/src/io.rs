//! 標準入出力とファイルの読み書きの補助

use alloc::string::String;
use alloc::vec::Vec;

use crate::{abi, sys};

/// fd を終わりまで読む
pub fn read_to_end(fd: i32) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = sys::read(fd, &mut buf);
        if n < 0 {
            return Err(-n);
        }
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
}

/// 全部書く
pub fn write_all(fd: i32, mut data: &[u8]) -> Result<(), i32> {
    while !data.is_empty() {
        let n = sys::write(fd, data);
        if n <= 0 {
            return Err(if n < 0 { -n } else { abi::err::EIO });
        }
        data = &data[n as usize..];
    }
    Ok(())
}

/// フィルタの入力: 引数のファイルを順に、無ければ標準入力を読み、(名前, 中身) を `f` に渡す。
/// "-" は標準入力。読めなかったファイルは「名前: 理由」を標準エラーに出して飛ばす。
/// 読めなかったものがあれば false
pub fn for_each_input(prog: &str, paths: &[String], mut f: impl FnMut(&str, &[u8])) -> bool {
    let mut ok = true;
    let stdin = [String::from("-")];
    let list = if paths.is_empty() { &stdin[..] } else { paths };
    for p in list {
        let data = if p == "-" { read_to_end(abi::STDIN) } else { crate::read_file(p) };
        match data {
            Ok(d) => f(if p == "-" { "(standard input)" } else { p }, &d),
            Err(e) => {
                crate::eprintln!("{prog}: {p}: {}", abi::err::name(e));
                ok = false;
            }
        }
    }
    ok
}
