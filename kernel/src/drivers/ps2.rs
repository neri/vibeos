//! 8042 PS/2 コントローラ（ポート 1 = キーボード、ポート 2 = マウス）
//!
//! 両方のポートの初期化をここで行う。割り込みハンドラの登録は keyboard / mouse が行う。

use crate::arch::port::{inb, outb};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 0x01;
const STATUS_INPUT_FULL: u8 = 0x02;

const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;
const CMD_DISABLE_PORT2: u8 = 0xA7;
const CMD_ENABLE_PORT2: u8 = 0xA8;
const CMD_TEST_PORT2: u8 = 0xA9;
const CMD_TEST_CONTROLLER: u8 = 0xAA;
const CMD_TEST_PORT1: u8 = 0xAB;
const CMD_DISABLE_PORT1: u8 = 0xAD;
const CMD_ENABLE_PORT1: u8 = 0xAE;
const CMD_WRITE_PORT2: u8 = 0xD4;

const CONFIG_PORT1_IRQ: u8 = 0x01;
const CONFIG_PORT2_IRQ: u8 = 0x02;
const CONFIG_PORT1_CLOCK_DISABLED: u8 = 0x10;
const CONFIG_PORT2_CLOCK_DISABLED: u8 = 0x20;
const CONFIG_TRANSLATION: u8 = 0x40;

const DEV_SET_DEFAULTS: u8 = 0xF6;
const DEV_ENABLE_SCANNING: u8 = 0xF4;
const DEV_ACK: u8 = 0xFA;

const TIMEOUT: u32 = 100_000;

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
        // SAFETY: 8042 / デバイスへのデータ
        unsafe { outb(DATA, value) };
    }
}

fn read_data() -> Option<u8> {
    // SAFETY: 出力バッファにデータがあるときだけ読む
    wait_output_full().then(|| unsafe { inb(DATA) })
}

/// 割り込みハンドラから: データポートを読む
pub fn read_data_now() -> u8 {
    // SAFETY: IRQ 1 / 12 はデータがあるときに来る
    unsafe { inb(DATA) }
}

fn write_port2(value: u8) {
    command(CMD_WRITE_PORT2);
    write_data(value);
}

/// どのデバイスがつながっていたか
pub struct Ports {
    pub keyboard: bool,
    pub mouse: bool,
}

/// コントローラと両方のデバイスを初期化する。割り込み禁止中に呼ぶこと
pub fn init() -> Ports {
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
        klog!("ps2: no controller");
        return Ports { keyboard: false, mouse: false };
    };
    config &= !(CONFIG_PORT1_IRQ | CONFIG_PORT2_IRQ);
    config |= CONFIG_TRANSLATION; // キーボードはセット 1 で受け取る
    command(CMD_WRITE_CONFIG);
    write_data(config);

    command(CMD_TEST_CONTROLLER);
    match read_data() {
        Some(0x55) => {}
        other => klog!("ps2: controller self test failed: {:?}", other),
    }
    // 自己テストで設定が初期化される機種があるので書き直す
    command(CMD_WRITE_CONFIG);
    write_data(config);

    // ポート 2 があるか: 有効にして設定の「ポート 2 のクロック無効」ビットが落ちれば 2 ポート
    command(CMD_ENABLE_PORT2);
    command(CMD_READ_CONFIG);
    let dual = read_data().is_some_and(|c| c & CONFIG_PORT2_CLOCK_DISABLED == 0);
    command(CMD_DISABLE_PORT2);

    command(CMD_TEST_PORT1);
    let keyboard = read_data() == Some(0x00);
    if !keyboard {
        klog!("ps2: port 1 test failed");
    }
    let mut mouse = false;
    if dual {
        command(CMD_TEST_PORT2);
        mouse = read_data() == Some(0x00);
        if !mouse {
            klog!("ps2: port 2 test failed");
        }
    }

    if keyboard {
        command(CMD_ENABLE_PORT1);
        write_data(DEV_ENABLE_SCANNING);
        if read_data() != Some(DEV_ACK) {
            klog!("ps2: keyboard did not acknowledge");
        }
    }
    if mouse {
        command(CMD_ENABLE_PORT2);
        write_port2(DEV_SET_DEFAULTS);
        let a = read_data();
        write_port2(DEV_ENABLE_SCANNING);
        let b = read_data();
        if a != Some(DEV_ACK) || b != Some(DEV_ACK) {
            klog!("ps2: mouse did not acknowledge ({:?}, {:?})", a, b);
            mouse = false;
        }
    }

    // 最初に読んだ設定は両ポート無効の状態なので、そのビットを落としてから割り込みを有効にする
    if keyboard {
        config &= !CONFIG_PORT1_CLOCK_DISABLED;
        config |= CONFIG_PORT1_IRQ;
    }
    if mouse {
        config &= !CONFIG_PORT2_CLOCK_DISABLED;
        config |= CONFIG_PORT2_IRQ;
    }
    command(CMD_WRITE_CONFIG);
    write_data(config);
    klog!("ps2: keyboard {}, mouse {}", keyboard, mouse);
    Ports { keyboard, mouse }
}
