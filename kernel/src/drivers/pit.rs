//! PIT（8253/8254 タイマ）: 100Hz の時刻の刻み

use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::port::outb;
use crate::arch::trap;
use crate::task::wait::WaitQueue;

const PIT_INPUT_HZ: u32 = 1_193_182;
pub const TICK_HZ: u32 = 100;

const CHANNEL0: u16 = 0x40;
const COMMAND: u16 = 0x43;
const CMD_CH0_LOHI_RATE: u8 = 0x34; // チャネル 0, 下位→上位バイト, モード 2（レートジェネレータ）

static TICKS: AtomicU32 = AtomicU32::new(0);
static TIMER_WAIT: WaitQueue = WaitQueue::new();

pub fn init() {
    let divisor = (PIT_INPUT_HZ + TICK_HZ / 2) / TICK_HZ;
    // SAFETY: PIT の設定
    unsafe {
        outb(COMMAND, CMD_CH0_LOHI_RATE);
        outb(CHANNEL0, divisor as u8);
        outb(CHANNEL0, (divisor >> 8) as u8);
    }
    trap::set_irq_handler(0, on_tick);
}

fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    TIMER_WAIT.notify_all();
}

/// 起動してからの刻み数（TICK_HZ 回で 1 秒）
pub fn ticks() -> u32 {
    TICKS.load(Ordering::Relaxed)
}

/// 少なくとも `ms` ミリ秒待つ
pub fn sleep_ms(ms: u32) {
    let start = ticks();
    let n = (ms * TICK_HZ).div_ceil(1000).max(1);
    TIMER_WAIT.wait_until(|| ticks().wrapping_sub(start) >= n);
}
