//! 待ち合わせ（WaitQueue）
//!
//! 「何かを待つ」処理は必ずこれを通す（DESIGN.md §6.4）。
//! 待つ側は条件が成り立っていなければ自分を待ち行列に入れて眠り、`notify_all` で起こされたら
//! 条件を見直す。条件の確認から眠るまでは割り込み禁止なので、その間の `notify_all` を取りこぼさない。

use alloc::collections::VecDeque;
use core::arch::asm;

use super::sched::{self, TaskId};
use crate::arch::cpu;
use crate::sync::IrqMutex;

pub struct WaitQueue {
    waiters: IrqMutex<VecDeque<TaskId>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        WaitQueue { waiters: IrqMutex::new(VecDeque::new()) }
    }

    /// `cond` が true を返すまで待つ。`cond` は割り込み禁止の状態で呼ばれる。
    ///
    /// ロックを持ったまま（= 割り込み禁止のまま）呼ぶと永久に起きられないので panic する。
    pub fn wait_until(&self, cond: impl FnMut() -> bool) {
        Self::wait_until_any(&[self], cond);
    }

    /// いくつかの待ち行列のどれかで起こされるたびに `cond` を見直し、true になるまで待つ
    /// （例: 入力か時間切れのどちらかを待つ）
    pub fn wait_until_any(queues: &[&WaitQueue], mut cond: impl FnMut() -> bool) {
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
            if sched::is_started() {
                let me = sched::current_id();
                for q in queues {
                    q.waiters.lock().push_back(me);
                }
                sched::block_current(); // 起こされると割り込み禁止のまま戻ってくる
                // 他の待ち行列に残った自分の ID は、そちらの notify_all で捨てられる（起こしても待っていなければ何もしない）
            } else {
                // スケジューラが動く前は、次の割り込みまで止まって見直す
                // SAFETY: 割り込みを許可して止まる（sti の直後の hlt までは割り込みが入らない）
                unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
            }
        }
    }

    /// 待っているタスクを全部起こす（割り込みハンドラからも呼んでよい。メモリ確保はしない）
    pub fn notify_all(&self) {
        loop {
            let next = self.waiters.lock().pop_front();
            match next {
                Some(id) => sched::wake(id),
                None => break,
            }
        }
    }
}
