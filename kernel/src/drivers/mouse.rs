//! PS/2 マウス（8042 のポート 2）
//!
//! 割り込みハンドラは生のバイトをリングバッファに積むだけ。パケットの解釈とカーソルの更新は
//! `input` のマウス用スレッドが行う。

use super::ps2;
use crate::arch::trap;
use crate::sync::IrqMutex;
use crate::task::wait::WaitQueue;
use crate::util::ring::Ring;

static RAW: IrqMutex<Ring<u8, 256>> = IrqMutex::new(Ring::new(0));
pub static RAW_WAIT: WaitQueue = WaitQueue::new();

/// IRQ 12 を有効にする（コントローラの初期化は ps2::init で済ませておく）
pub fn init() {
    trap::set_irq_handler(12, on_irq);
}

fn on_irq() {
    let byte = ps2::read_data_now();
    RAW.lock().push(byte); // あふれたら捨てる（パケットの区切りは解釈側で合わせ直す）
    RAW_WAIT.notify_all();
}

pub fn pop_raw() -> Option<u8> {
    RAW.lock().pop()
}

pub fn has_raw() -> bool {
    !RAW.lock().is_empty()
}
