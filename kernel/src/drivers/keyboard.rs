//! PS/2 キーボード（8042 のポート 1）
//!
//! 割り込みハンドラは生のスキャンコードをリングバッファに積むだけ。
//! スキャンコードの解釈（vibe-keyboard）は読む側で行う。

use core::sync::atomic::{AtomicBool, Ordering};

use vibe_keyboard::{Decoder, KeyEvent, US, UsLayout};

use super::ps2;
use crate::arch::trap;

use crate::sync::IrqMutex;
use crate::task::wait::WaitQueue;
use crate::util::ring::Ring;

static RAW: IrqMutex<Ring<u8, 128>> = IrqMutex::new(Ring::new(0));
static DECODER: IrqMutex<Decoder<UsLayout>> = IrqMutex::new(Decoder::new(&US));
static KEY_WAIT: WaitQueue = WaitQueue::new();

/// IRQ 1 を有効にする（コントローラの初期化は ps2::init で済ませておく）
pub fn init() {
    trap::set_irq_handler(1, on_irq);
}

/// Ctrl+C を割り込みの中で見つけるための最小限の状態（文字への変換は読む側で行う）
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static AFTER_E0: AtomicBool = AtomicBool::new(false);

fn watch_ctrl_c(byte: u8) {
    let e0 = AFTER_E0.swap(byte == 0xE0, Ordering::Relaxed);
    match byte & 0x7F {
        0x1D => CTRL_DOWN.store(byte & 0x80 == 0, Ordering::Relaxed), // 左 Ctrl（E0 付きは右 Ctrl）
        0x2E if byte & 0x80 == 0 && !e0 && CTRL_DOWN.load(Ordering::Relaxed) => crate::process::request_interrupt(),
        _ => {}
    }
}

fn on_irq() {
    let byte = ps2::read_data_now();
    watch_ctrl_c(byte);
    if !RAW.lock().push(byte) {
        klog!("keyboard: buffer full, dropped {:#x}", byte);
    }
    KEY_WAIT.notify_all();
}

/// 届いているキーイベントがあれば返す（待たない）
pub fn try_read_event() -> Option<KeyEvent> {
    loop {
        let byte = RAW.lock().pop()?;
        if let Some(e) = DECODER.lock().feed(byte) {
            return Some(e);
        }
    }
}

/// キーイベントが来るまで待つ
pub fn read_event() -> KeyEvent {
    loop {
        if let Some(e) = try_read_event() {
            return e;
        }
        KEY_WAIT.wait_until(|| !RAW.lock().is_empty());
    }
}
