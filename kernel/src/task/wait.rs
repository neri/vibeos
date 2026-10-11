//! 待ち合わせ（WaitQueue）
//!
//! 「何かを待つ」処理は必ずこれを通す（DESIGN.md §6.4）。
//!
//! **仮実装**: スケジューラが入るまでは、待つ側は `hlt` で次の割り込みまで止まり、
//! 起きるたびに条件を見直す。`notify_all` は何もしない。Phase 5 で、待っているタスクを
//! 眠らせて `notify_all` で起こす本物の実装に差し替える（呼び出し側は変えない）。

use core::arch::asm;

use crate::arch::cpu;

pub struct WaitQueue {
    _private: (),
}

impl WaitQueue {
    pub const fn new() -> Self {
        WaitQueue { _private: () }
    }

    /// `cond` が true を返すまで待つ。`cond` は割り込み禁止の状態で呼ばれる。
    ///
    /// ロックを持ったまま（= 割り込み禁止のまま）呼ぶと永久に起きられないので panic する。
    pub fn wait_until(&self, mut cond: impl FnMut() -> bool) {
        if !cpu::interrupts_enabled() {
            panic!("WaitQueue::wait_until called with interrupts disabled (holding a lock?)");
        }
        loop {
            cpu::disable_interrupts();
            if cond() {
                // SAFETY: 呼ばれたときは許可されていた
                unsafe { cpu::enable_interrupts() };
                return;
            }
            // sti の直後の 1 命令（hlt）が終わるまでは割り込みが入らないので、
            // 条件を見てから止まるまでの間に来た割り込みを取りこぼさない
            // SAFETY: 割り込みを許可して止まるだけ
            unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
        }
    }

    /// 待っている人を起こす（割り込みハンドラからも呼んでよい）
    pub fn notify_all(&self) {
        // 仮実装では何もしない（待つ側は割り込みのたびに条件を見直している）
    }
}
