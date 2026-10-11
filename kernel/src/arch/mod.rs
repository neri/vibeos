//! x86 の CPU 命令・CPU の表・割り込み。`unsafe` なハードウェア操作はこの層に閉じ込める。

pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod pic;
pub mod port;
pub mod trap;
pub mod usermode;
