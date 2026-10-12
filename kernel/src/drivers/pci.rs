//! PCI のバス（コンフィグ空間の読み書きと機器の一覧）
//!
//! コンフィグ空間へは「方式 1」（ポート 0xCF8 に番地を書き、0xCFC で読み書きする）で触る。
//! 0xCF8 に書いた値が読み戻せなければ PCI は無い（ISA だけの PC）とみなす。
//! 割り込みの番号は BIOS が決めてコンフィグ空間（0x3C）に書いたものを使う。

use alloc::vec::Vec;

use crate::arch::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;
const ENABLE: u32 = 0x8000_0000;

/// コマンドレジスタ: I/O 空間・バスマスタ（DMA）を有効にする
pub const COMMAND_IO: u16 = 0x01;
pub const COMMAND_BUS_MASTER: u16 = 0x04;

#[derive(Debug, Clone, Copy)]
pub struct Device {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub irq: u8,
    pub bars: [u32; 6],
}

impl Device {
    /// I/O 空間の BAR ならその先頭のポート
    pub fn io_base(&self, bar: usize) -> Option<u16> {
        let v = self.bars[bar];
        (v & 1 == 1 && v & !3 != 0).then(|| (v & 0xFFFC) as u16)
    }

    /// メモリ空間の BAR ならその番地
    pub fn mem_base(&self, bar: usize) -> Option<u32> {
        let v = self.bars[bar];
        (v & 1 == 0 && v & !0xF != 0).then_some(v & !0xF)
    }

    fn read16(&self, off: u8) -> u16 {
        let v = read32(self.bus, self.dev, self.func, off & !3);
        (v >> ((off & 2) * 8)) as u16
    }

    /// コマンドレジスタにビットを立てる
    pub fn enable(&self, bits: u16) {
        let cmd = self.read16(0x04) | bits;
        // 0x04 の上位 16bit はステータス（1 を書くと消える）なので 0 を書く
        write32(self.bus, self.dev, self.func, 0x04, cmd as u32);
    }
}

fn address(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    ENABLE | (bus as u32) << 16 | (dev as u32) << 11 | (func as u32) << 8 | (off & 0xFC) as u32
}

pub fn read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    // SAFETY: PCI のコンフィグ空間の番地を書いてデータを読む（present() で方式 1 があると確かめてから使う）
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, dev, func, off));
        inl(CONFIG_DATA)
    }
}

fn write32(bus: u8, dev: u8, func: u8, off: u8, v: u32) {
    // SAFETY: 同上
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, dev, func, off));
        outl(CONFIG_DATA, v);
    }
}

/// PCI の方式 1 があるか
pub fn present() -> bool {
    // SAFETY: 0xCF8 は PCI のコンフィグ番地のレジスタ。無い機械では書いても何も起きず、0xFFFFFFFF が読める
    unsafe {
        let old = inl(CONFIG_ADDRESS);
        outl(CONFIG_ADDRESS, ENABLE);
        let ok = inl(CONFIG_ADDRESS) == ENABLE;
        outl(CONFIG_ADDRESS, old);
        ok
    }
}

/// すべての機器（バス 0〜255 を総当たり）
pub fn scan() -> Vec<Device> {
    let mut out = Vec::new();
    if !present() {
        return out;
    }
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            let id = read32(bus, dev, 0, 0);
            if id & 0xFFFF == 0xFFFF {
                continue;
            }
            // ヘッダの種類の bit7 が立っていれば機能（function）が複数ある
            let multi = read32(bus, dev, 0, 0x0C) >> 16 & 0x80 != 0;
            for func in 0..if multi { 8 } else { 1 } {
                let id = read32(bus, dev, func, 0);
                if id & 0xFFFF == 0xFFFF {
                    continue;
                }
                let class = read32(bus, dev, func, 0x08);
                let header = (read32(bus, dev, func, 0x0C) >> 16) as u8 & 0x7F;
                let mut bars = [0u32; 6];
                // BAR は普通の機器（ヘッダの種類 0）だけ読む
                if header == 0 {
                    for (i, b) in bars.iter_mut().enumerate() {
                        *b = read32(bus, dev, func, 0x10 + i as u8 * 4);
                    }
                }
                out.push(Device {
                    bus,
                    dev,
                    func,
                    vendor: id as u16,
                    device: (id >> 16) as u16,
                    class: (class >> 24) as u8,
                    subclass: (class >> 16) as u8,
                    irq: read32(bus, dev, func, 0x3C) as u8,
                    bars,
                });
            }
        }
    }
    out
}
