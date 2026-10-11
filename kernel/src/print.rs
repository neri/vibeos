//! 出力マクロ
//!
//! - `kprint!` / `kprintln!`: 画面のコンソールとシリアルの両方に出す
//! - `klog!`: シリアルだけに出す（テストの判定やデバッグ用）

use core::fmt;

use crate::drivers::serial;
use crate::gfx::console;

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => { $crate::print::print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($($arg:tt)*) => { $crate::print::print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => { $crate::print::log(format_args!("{}\n", format_args!($($arg)*))) };
}

pub fn print(args: fmt::Arguments) {
    serial::write_fmt(args);
    console::write_fmt(args);
}

pub fn log(args: fmt::Arguments) {
    serial::write_fmt(args);
}
