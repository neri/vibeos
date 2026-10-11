//! 再起動

use core::arch::asm;

use crate::arch::cpu;
use crate::arch::port::{inb, outb};

/// キーボードコントローラのリセット線で再起動する。だめなら空の IDT で三重フォルトを起こす
pub fn reboot() -> ! {
    cpu::disable_interrupts();
    // SAFETY: 8042 のコマンド 0xFE は CPU のリセット線を下げる
    unsafe {
        for _ in 0..100_000 {
            if inb(0x64) & 0x02 == 0 {
                break;
            }
        }
        outb(0x64, 0xFE);
    }
    for _ in 0..1_000_000 {
        core::hint::spin_loop();
    }
    // SAFETY: 大きさ 0 の IDT を読み込んで例外を起こすと、処理できずに三重フォルト → リセット
    unsafe {
        let null_idt = [0u16; 3];
        asm!("lidt [{}]", "int3", in(reg) &null_idt, options(noreturn));
    }
}
