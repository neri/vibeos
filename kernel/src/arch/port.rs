//! I/O ポート

#![allow(dead_code)]

use core::arch::asm;

/// # Safety
/// ポートへの書き込みはハードウェアの状態を変える。呼び出し側がデバイスの仕様に沿うこと。
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: 呼び出し側の責任（関数の Safety 参照）
    unsafe { asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags)) }
}

/// # Safety
/// ポートによっては読むだけで状態が変わる。
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags)) }
    value
}

/// # Safety
/// `outb` と同じ。
pub unsafe fn outw(port: u16, value: u16) {
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags)) }
}

/// # Safety
/// `inb` と同じ。
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags)) }
    value
}

/// # Safety
/// `outb` と同じ。
pub unsafe fn outl(port: u16, value: u32) {
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags)) }
}

/// # Safety
/// `inb` と同じ。
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags)) }
    value
}

/// 古いデバイスのための短い待ち（未使用のポート 0x80 に書く。約 1µs）
pub fn io_wait() {
    // SAFETY: ポート 0x80 は POST コード用で、書いても副作用がない
    unsafe { outb(0x80, 0) }
}
