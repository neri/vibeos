#![no_std]
#![no_main]

use core::panic::PanicInfo;

core::arch::global_asm!(
    ".section .text.entry, \"ax\"",
    ".global _start",
    "_start:",
    "2:",
    "hlt",
    "jmp 2b",
);

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
