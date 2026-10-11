//! 割り込み禁止付きのロック
//!
//! CPU は 1 つなので、ロックの実体は「持っている間は割り込みを止める」こと。
//! 割り込みもタスク切り替えも起きないので、持っている間に他の誰かが同じロックを
//! 取りに来ることはない。取りに来たならそれは同じ流れでの二重ロック（バグ）なので panic する。
//!
//! 複数のロックを同時に持つときは、取る順番をそろえること（例: CONSOLE → SCREEN）。
//!
//! 長く持つ可能性のあるもの（ファイルシステムなど、待つかもしれない処理）には、割り込みを
//! 止めない `Mutex`（取れなければ眠って待つ）を使う。

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::cpu;
use crate::task::sched::{self, TaskId};
use crate::task::wait::WaitQueue;

pub struct IrqMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: 中身へのアクセスはロック（割り込み禁止 + 二重取得の検出）で 1 つに限られる
unsafe impl<T: Send> Sync for IrqMutex<T> {}

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        IrqMutex { locked: AtomicBool::new(false), data: UnsafeCell::new(value) }
    }

    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        match self.try_lock() {
            Some(g) => g,
            None => panic!("IrqMutex locked twice (deadlock)"),
        }
    }

    pub fn try_lock(&self) -> Option<IrqMutexGuard<'_, T>> {
        let was_enabled = cpu::interrupts_enabled();
        cpu::disable_interrupts();
        if self.locked.swap(true, Ordering::Acquire) {
            if was_enabled {
                // SAFETY: 呼び出し前は許可されていたので元に戻すだけ
                unsafe { cpu::enable_interrupts() };
            }
            return None;
        }
        Some(IrqMutexGuard { mutex: self, was_enabled })
    }

    /// ロックを無視して中身を返す。panic 処理の中でだけ使う。
    ///
    /// # Safety
    /// 他の誰も中身を使っていない（またはもう二度と戻ってこない）こと。
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn force_get(&self) -> &mut T {
        // SAFETY: 呼び出し側の責任
        unsafe { &mut *self.data.get() }
    }
}

pub struct IrqMutexGuard<'a, T> {
    mutex: &'a IrqMutex<T>,
    was_enabled: bool,
}

impl<T> Deref for IrqMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: ガードがある間はこの参照だけが中身に触れる
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for IrqMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 同上
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for IrqMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
        if self.was_enabled {
            // SAFETY: ロックを取る前は許可されていたので元に戻すだけ
            unsafe { cpu::enable_interrupts() };
        }
    }
}

/// 起動時に一度だけ書き、以降は CPU（GDT・IDT など）から参照されるだけの静的データ用。
///
/// ロックを持たないので、書くのは割り込み禁止中の初期化処理の中だけにすること。
pub struct BootCell<T>(UnsafeCell<T>);

// SAFETY: 書き込みは単一の初期化処理の中だけ（型の説明参照）
unsafe impl<T> Sync for BootCell<T> {}

impl<T> BootCell<T> {
    pub const fn new(value: T) -> Self {
        BootCell(UnsafeCell::new(value))
    }

    pub fn as_ptr(&self) -> *mut T {
        self.0.get()
    }
}

/// 眠って待つロック。持っている間も割り込みは止めないので、中で待つ処理（I/O など）をしてよい。
/// 割り込みハンドラの中では使えない。
pub struct Mutex<T> {
    locked: AtomicBool,
    /// 持っているタスク（同じタスクでの二重取得を見つけるため）
    owner: core::sync::atomic::AtomicU32,
    waiters: WaitQueue,
    data: UnsafeCell<T>,
}

// SAFETY: 中身へのアクセスはロックで 1 つに限られる
unsafe impl<T: Send> Sync for Mutex<T> {}
// SAFETY: 同上
unsafe impl<T: Send> Send for Mutex<T> {}

const NO_OWNER: u32 = u32::MAX;

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Mutex {
            locked: AtomicBool::new(false),
            owner: core::sync::atomic::AtomicU32::new(NO_OWNER),
            waiters: WaitQueue::new(),
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        let me: TaskId = if sched::is_started() { sched::current_id() } else { 0 };
        loop {
            if !self.locked.swap(true, Ordering::Acquire) {
                self.owner.store(me, Ordering::Relaxed);
                return MutexGuard { mutex: self };
            }
            if self.owner.load(Ordering::Relaxed) == me {
                panic!("Mutex locked twice by task {me} (deadlock)");
            }
            self.waiters.wait_until(|| !self.locked.load(Ordering::Relaxed));
        }
    }
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: ガードがある間はこの参照だけが中身に触れる
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 同上
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.owner.store(NO_OWNER, Ordering::Relaxed);
        self.mutex.locked.store(false, Ordering::Release);
        self.mutex.waiters.notify_all();
    }
}
