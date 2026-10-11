//! PC スピーカー（PIT のチャネル 2 で矩形波を作る）
//!
//! `play` は音を出し始めて、止める時刻を覚えておくだけ（待たない）。タイマの割り込みが時刻を見て止める。

use core::sync::atomic::{AtomicU32, Ordering};

use super::pit;
use crate::arch::port::{inb, outb};

const PIT_INPUT_HZ: u32 = 1_193_182;
const PIT_CH2: u16 = 0x42;
const PIT_CMD: u16 = 0x43;
const PORT_B: u16 = 0x61;
const SPEAKER_BITS: u8 = 0x03; // ゲート 2 と スピーカーの出力

/// 止める時刻（タイマの刻み）。0 なら鳴っていない
static STOP_AT: AtomicU32 = AtomicU32::new(0);

/// `freq` Hz の音を `ms` ミリ秒鳴らす（待たない）。`freq` が 0 なら止める
pub fn play(freq: u32, ms: u32) {
    if freq == 0 || ms == 0 {
        off();
        return;
    }
    let div = (PIT_INPUT_HZ / freq.clamp(20, 20_000)) as u16;
    // SAFETY: PIT のチャネル 2 とスピーカーのゲートを設定する
    unsafe {
        outb(PIT_CMD, 0xB6); // チャネル 2, 下位→上位, モード 3（矩形波）
        outb(PIT_CH2, div as u8);
        outb(PIT_CH2, (div >> 8) as u8);
        outb(PORT_B, inb(PORT_B) | SPEAKER_BITS);
    }
    let ticks = (ms * pit::TICK_HZ).div_ceil(1000).max(1);
    STOP_AT.store(pit::ticks().wrapping_add(ticks).max(1), Ordering::Relaxed);
}

pub fn off() {
    STOP_AT.store(0, Ordering::Relaxed);
    // SAFETY: スピーカーのゲートを閉じる
    unsafe { outb(PORT_B, inb(PORT_B) & !SPEAKER_BITS) };
}

/// タイマ割り込みから: 止める時刻を過ぎていたら止める
pub fn on_tick(now: u32) {
    let stop = STOP_AT.load(Ordering::Relaxed);
    if stop != 0 && now.wrapping_sub(stop) < u32::MAX / 2 {
        off();
    }
}
