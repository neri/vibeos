//! 8259A PIC（割り込みコントローラ）
//!
//! IRQ 0〜15 をベクタ 0x20〜0x2F に付け替える（初期値は CPU 例外と重なっている）。

use super::idt::IRQ_BASE;
use super::port::{inb, io_wait, outb};

const MASTER_CMD: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_CMD: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;

const ICW1_INIT_ICW4: u8 = 0x11;
const ICW4_8086: u8 = 0x01;
const OCW3_READ_ISR: u8 = 0x0B;
const EOI: u8 = 0x20;
const CASCADE_IRQ: u8 = 2;

/// 割り込み禁止中に 1 回だけ呼ぶ。全 IRQ をマスクした状態で終わる（カスケードの IRQ 2 だけ有効）
pub fn init() {
    // SAFETY: PIC の標準的な初期化手順
    unsafe {
        outb(MASTER_CMD, ICW1_INIT_ICW4);
        io_wait();
        outb(SLAVE_CMD, ICW1_INIT_ICW4);
        io_wait();
        outb(MASTER_DATA, IRQ_BASE); // ICW2: ベクタの先頭
        io_wait();
        outb(SLAVE_DATA, IRQ_BASE + 8);
        io_wait();
        outb(MASTER_DATA, 1 << CASCADE_IRQ); // ICW3: スレーブは IRQ 2 につながっている
        io_wait();
        outb(SLAVE_DATA, CASCADE_IRQ);
        io_wait();
        outb(MASTER_DATA, ICW4_8086);
        io_wait();
        outb(SLAVE_DATA, ICW4_8086);
        io_wait();
        outb(MASTER_DATA, !(1 << CASCADE_IRQ));
        outb(SLAVE_DATA, 0xFF);
    }
}

pub fn unmask(irq: u8) {
    let (port, bit) = if irq < 8 { (MASTER_DATA, irq) } else { (SLAVE_DATA, irq - 8) };
    // SAFETY: マスクレジスタの 1 ビットを変えるだけ
    unsafe { outb(port, inb(port) & !(1 << bit)) };
}

#[allow(dead_code)]
pub fn mask(irq: u8) {
    let (port, bit) = if irq < 8 { (MASTER_DATA, irq) } else { (SLAVE_DATA, irq - 8) };
    // SAFETY: 同上
    unsafe { outb(port, inb(port) | (1 << bit)) };
}

pub fn end_of_interrupt(irq: u8) {
    // SAFETY: EOI の送信
    unsafe {
        if irq >= 8 {
            outb(SLAVE_CMD, EOI);
        }
        outb(MASTER_CMD, EOI);
    }
}

/// IRQ 7 / 15 は雑音などで「実際には無い割り込み」が来ることがある。
/// そのときは処理せず、EOI も（スレーブ分は）送らない。
pub fn is_spurious(irq: u8) -> bool {
    // SAFETY: ISR（処理中の割り込み）レジスタを読むだけ
    unsafe {
        match irq {
            7 => {
                outb(MASTER_CMD, OCW3_READ_ISR);
                inb(MASTER_CMD) & 0x80 == 0
            }
            15 => {
                outb(SLAVE_CMD, OCW3_READ_ISR);
                if inb(SLAVE_CMD) & 0x80 == 0 {
                    outb(MASTER_CMD, EOI); // マスタ側は本物のカスケード割り込みだった
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

/// NMI を許可する（ローダが禁止したまま渡してくる）
pub fn enable_nmi() {
    // SAFETY: CMOS インデックスポートの bit7 を落とすだけ
    unsafe {
        outb(0x70, inb(0x70) & 0x7F);
        inb(0x71);
    }
}
