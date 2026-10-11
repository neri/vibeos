//! CMOS の時計（RTC）
//!
//! 起動時に 1 回だけ読み、その後はタイマの刻みを足して今の時刻にする（CMOS を何度も読まない）。
//! PC の時計はふつう現地時刻なので、タイムゾーンは扱わない。

use core::sync::atomic::{AtomicU32, Ordering};

use vibe_time::DateTime;

use super::pit;
use crate::arch::port::{inb, outb};

const CMOS_INDEX: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;
const STATUS_A_UPDATING: u8 = 0x80;
const STATUS_B_BINARY: u8 = 0x04;
const STATUS_B_24H: u8 = 0x02;

/// 起動時の時刻（1970 年からの秒）と、そのときのタイマの刻み
static BOOT_SECS: AtomicU32 = AtomicU32::new(0);
static BOOT_TICKS: AtomicU32 = AtomicU32::new(0);

fn read(reg: u8) -> u8 {
    // SAFETY: CMOS の読み出し（NMI 禁止ビットは立てない）
    unsafe {
        outb(CMOS_INDEX, reg);
        inb(CMOS_DATA)
    }
}

/// 時計の値をそのまま読む（更新中は待つ）
fn read_raw() -> [u8; 6] {
    for _ in 0..100_000 {
        if read(0x0A) & STATUS_A_UPDATING == 0 {
            break;
        }
    }
    [read(0x00), read(0x02), read(0x04), read(0x07), read(0x08), read(0x09)]
}

pub fn init() {
    // 読んでいる途中で繰り上がると値が崩れるので、2 回続けて同じ値になるまで読む
    let mut raw = read_raw();
    for _ in 0..5 {
        let again = read_raw();
        if again == raw {
            break;
        }
        raw = again;
    }
    let status_b = read(0x0B);
    let bcd = |v: u8| if status_b & STATUS_B_BINARY != 0 { v } else { (v & 0x0F) + (v >> 4) * 10 };
    let [s, m, h, day, month, year] = raw;
    let mut hour = bcd(h & 0x7F);
    if status_b & STATUS_B_24H == 0 {
        // 12 時間表示: bit7 が午後
        hour %= 12;
        if h & 0x80 != 0 {
            hour += 12;
        }
    }
    let year = 2000 + bcd(year) as u16; // 世紀のレジスタは機種によって無いので 20xx 年とする
    match DateTime::new(year, bcd(month), bcd(day), hour, bcd(m), bcd(s)) {
        Some(t) => {
            BOOT_SECS.store(t.to_unix() as u32, Ordering::Relaxed);
            BOOT_TICKS.store(pit::ticks(), Ordering::Relaxed);
            klog!("rtc: {}", t);
        }
        None => klog!("rtc: invalid time {:02x?}", raw),
    }
}

/// 今の時刻（1970 年からの秒）。時計が読めなければ 0
pub fn now_secs() -> u32 {
    let boot = BOOT_SECS.load(Ordering::Relaxed);
    if boot == 0 {
        return 0;
    }
    boot + pit::ticks().wrapping_sub(BOOT_TICKS.load(Ordering::Relaxed)) / pit::TICK_HZ
}

/// 今の時刻。時計が読めなければ None
pub fn now() -> Option<DateTime> {
    match now_secs() {
        0 => None,
        s => Some(DateTime::from_unix(s as u64)),
    }
}
