//! x86 の CPU 命令とポート I/O。`unsafe` なハードウェア操作はこの層に閉じ込める。

pub mod cpu;
pub mod port;
