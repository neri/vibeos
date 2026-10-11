//! QEMU のテスト用デバイス（qemu-test 機能のときだけ使う）

use crate::arch::port::outl;

const ISA_DEBUG_EXIT: u16 = 0xF4;

/// QEMU の終了コードは (code << 1) | 1 になる。成功 = 33, 失敗 = 35
#[allow(dead_code)]
pub const EXIT_SUCCESS: u32 = 0x10;
pub const EXIT_FAILURE: u32 = 0x11;

#[allow(dead_code)]
pub fn exit(code: u32) -> ! {
    // SAFETY: テスト用ビルドでは QEMU に isa-debug-exit が付いている前提
    unsafe { outl(ISA_DEBUG_EXIT, code) };
    crate::arch::cpu::halt_forever()
}
