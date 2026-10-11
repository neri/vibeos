//! CPU の制御命令

use core::arch::asm;

const EFLAGS_IF: u32 = 1 << 9;

pub fn interrupts_enabled() -> bool {
    let flags: u32;
    // SAFETY: EFLAGS を読むだけ
    unsafe { asm!("pushfd", "pop {}", out(reg) flags, options(nomem, preserves_flags)) }
    flags & EFLAGS_IF != 0
}

/// 割り込みを禁止する
pub fn disable_interrupts() {
    // SAFETY: 割り込み禁止は常に安全（解除し忘れはロックのガードが面倒を見る）
    unsafe { asm!("cli", options(nomem, nostack)) }
}

/// 割り込みを許可する
///
/// # Safety
/// IDT が用意されていて、割り込まれても壊れない状態であること。
pub unsafe fn enable_interrupts() {
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("sti", options(nomem, nostack)) }
}

/// 次の割り込みまで止まる
pub fn halt() {
    // SAFETY: hlt は割り込みが来れば戻る。割り込み禁止中なら永久に止まるが、それは呼び出し側の意図
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
}

/// 割り込みを禁止して永久に止まる
pub fn halt_forever() -> ! {
    loop {
        disable_interrupts();
        halt();
    }
}
