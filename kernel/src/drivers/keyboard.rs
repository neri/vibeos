//! PS/2 キーボード（8042 コントローラの第 1 ポート）
//!
//! 割り込みハンドラは生のスキャンコードをリングバッファに積むだけ。
//! スキャンコードの解釈（vibe-keyboard）は読む側で行う。

use vibe_keyboard::{Decoder, KeyEvent, US, UsLayout};

use crate::arch::port::{inb, outb};
use crate::arch::trap;
use crate::sync::IrqMutex;
use crate::task::wait::WaitQueue;
use crate::util::ring::Ring;

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 0x01;
const STATUS_INPUT_FULL: u8 = 0x02;

const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;
const CMD_DISABLE_PORT2: u8 = 0xA7;
const CMD_TEST_CONTROLLER: u8 = 0xAA;
const CMD_TEST_PORT1: u8 = 0xAB;
const CMD_DISABLE_PORT1: u8 = 0xAD;
const CMD_ENABLE_PORT1: u8 = 0xAE;

const CONFIG_PORT1_IRQ: u8 = 0x01;
const CONFIG_PORT2_IRQ: u8 = 0x02;
const CONFIG_PORT1_CLOCK_DISABLED: u8 = 0x10;
const CONFIG_TRANSLATION: u8 = 0x40;

const KBD_ENABLE_SCANNING: u8 = 0xF4;
const KBD_ACK: u8 = 0xFA;

const TIMEOUT: u32 = 100_000;

static RAW: IrqMutex<Ring<u8, 128>> = IrqMutex::new(Ring::new(0));
static DECODER: IrqMutex<Decoder<UsLayout>> = IrqMutex::new(Decoder::new(&US));
static KEY_WAIT: WaitQueue = WaitQueue::new();

fn wait_input_empty() -> bool {
    // SAFETY: ステータスを読むだけ
    (0..TIMEOUT).any(|_| unsafe { inb(STATUS) } & STATUS_INPUT_FULL == 0)
}

fn wait_output_full() -> bool {
    // SAFETY: 同上
    (0..TIMEOUT).any(|_| unsafe { inb(STATUS) } & STATUS_OUTPUT_FULL != 0)
}

fn command(cmd: u8) {
    if wait_input_empty() {
        // SAFETY: 8042 へのコマンド
        unsafe { outb(COMMAND, cmd) };
    }
}

fn write_data(value: u8) {
    if wait_input_empty() {
        // SAFETY: 8042 / キーボードへのデータ
        unsafe { outb(DATA, value) };
    }
}

fn read_data() -> Option<u8> {
    // SAFETY: 出力バッファにデータがあるときだけ読む
    wait_output_full().then(|| unsafe { inb(DATA) })
}

/// 8042 を初期化してキーボードの割り込みを有効にする。割り込み禁止中に呼ぶこと
pub fn init() {
    command(CMD_DISABLE_PORT1);
    command(CMD_DISABLE_PORT2);
    // 出力バッファに残っているものを捨てる
    for _ in 0..64 {
        // SAFETY: ステータスとデータを読むだけ
        unsafe {
            if inb(STATUS) & STATUS_OUTPUT_FULL == 0 {
                break;
            }
            inb(DATA);
        }
    }

    command(CMD_READ_CONFIG);
    let Some(mut config) = read_data() else {
        klog!("keyboard: no PS/2 controller");
        return;
    };
    config &= !(CONFIG_PORT1_IRQ | CONFIG_PORT2_IRQ);
    config |= CONFIG_TRANSLATION; // セット 1 で受け取る
    command(CMD_WRITE_CONFIG);
    write_data(config);

    command(CMD_TEST_CONTROLLER);
    match read_data() {
        Some(0x55) => {}
        other => klog!("keyboard: controller self test failed: {:?}", other),
    }
    // 自己テストで設定が初期化される機種があるので書き直す
    command(CMD_WRITE_CONFIG);
    write_data(config);

    command(CMD_TEST_PORT1);
    match read_data() {
        Some(0x00) => {}
        other => klog!("keyboard: port 1 test failed: {:?}", other),
    }

    command(CMD_ENABLE_PORT1);
    write_data(KBD_ENABLE_SCANNING);
    match read_data() {
        Some(KBD_ACK) => {}
        other => klog!("keyboard: enable scanning not acknowledged: {:?}", other),
    }

    // 最初に読んだ設定は「ポート 1 無効」の状態なので、そのビットを落としてから書く
    config &= !CONFIG_PORT1_CLOCK_DISABLED;
    config |= CONFIG_PORT1_IRQ;
    command(CMD_WRITE_CONFIG);
    write_data(config);

    trap::set_irq_handler(1, on_irq);
    klog!("keyboard: ready");
}

fn on_irq() {
    // SAFETY: IRQ 1 はデータがあるときに来る
    let byte = unsafe { inb(DATA) };
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

/// 文字が入力されるまで待つ
pub fn read_char() -> char {
    loop {
        if let KeyEvent { pressed: true, ch: Some(c), .. } = read_event() {
            return c;
        }
    }
}
