//! COM1 シリアルポート（デバッグログ用）
//!
//! UART が無い実機でも止まらないよう、初期化時にループバックで存在を確かめ、
//! 送信待ちには上限を付ける。

use core::fmt::{self, Write};

use crate::arch::port::{inb, outb};
use crate::sync::IrqMutex;

const COM1: u16 = 0x3F8;
const DATA: u16 = COM1;
const INT_ENABLE: u16 = COM1 + 1;
const FIFO_CTRL: u16 = COM1 + 2;
const LINE_CTRL: u16 = COM1 + 3;
const MODEM_CTRL: u16 = COM1 + 4;
const LINE_STATUS: u16 = COM1 + 5;

const LSR_TX_EMPTY: u8 = 0x20;

pub struct Serial {
    present: bool,
}

static SERIAL: IrqMutex<Serial> = IrqMutex::new(Serial { present: false });

pub fn init() {
    let mut s = SERIAL.lock();
    // SAFETY: COM1 の標準的な初期化手順。UART が無ければ書き込みは無視される
    s.present = unsafe {
        outb(INT_ENABLE, 0x00);
        outb(LINE_CTRL, 0x80); // DLAB
        outb(DATA, 0x01); // 115200 bps
        outb(INT_ENABLE, 0x00);
        outb(LINE_CTRL, 0x03); // 8N1
        outb(FIFO_CTRL, 0xC7);
        outb(MODEM_CTRL, 0x1E); // ループバックで存在確認
        outb(DATA, 0xAE);
        let ok = inb(DATA) == 0xAE;
        outb(MODEM_CTRL, 0x03); // 通常動作（DTR, RTS）
        ok
    };
}

impl Serial {
    fn put_byte(&mut self, b: u8) {
        if !self.present {
            return;
        }
        // SAFETY: 送信バッファが空くのを待ってからデータレジスタに書く
        unsafe {
            for _ in 0..100_000 {
                if inb(LINE_STATUS) & LSR_TX_EMPTY != 0 {
                    break;
                }
            }
            outb(DATA, b);
        }
    }
}

impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if b == b'\n' {
                self.put_byte(b'\r');
            }
            self.put_byte(b);
        }
        Ok(())
    }
}

pub fn write_fmt(args: fmt::Arguments) {
    let _ = SERIAL.lock().write_fmt(args);
}

/// panic 処理用: ロックを無視して書く
///
/// # Safety
/// panic 処理の中からだけ呼ぶこと（以降、通常の処理には戻らない）。
pub unsafe fn panic_write_fmt(args: fmt::Arguments) {
    // SAFETY: 呼び出し側の責任
    let _ = unsafe { SERIAL.force_get() }.write_fmt(args);
}
