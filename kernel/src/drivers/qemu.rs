//! QEMU のテスト用デバイス（qemu-test 機能のときだけ使う）
//!
//! Bochs には isa-debug-exit が無いので、続けて Bochs の終了用ポート（0x8900 に "Shutdown"）にも書く。
//! Bochs では終了コードは伝わらない（成否はシリアルの出力で判断する）。

use crate::arch::port::{outb, outl};

const BOCHS_SHUTDOWN: u16 = 0x8900;

const ISA_DEBUG_EXIT: u16 = 0xF4;

/// QEMU の終了コードは (code << 1) | 1 になる。成功 = 33, 失敗 = 35
#[allow(dead_code)]
pub const EXIT_SUCCESS: u32 = 0x10;
pub const EXIT_FAILURE: u32 = 0x11;

#[allow(dead_code)]
pub fn exit(code: u32) -> ! {
    // SAFETY: テスト用ビルドでは QEMU に isa-debug-exit が付いている前提。Bochs ではそのポートは何もしない
    unsafe { outl(ISA_DEBUG_EXIT, code) };
    for b in b"Shutdown" {
        // SAFETY: Bochs の終了用ポート。QEMU ではここまで来ない
        unsafe { outb(BOCHS_SHUTDOWN, *b) };
    }
    crate::arch::cpu::halt_forever()
}
