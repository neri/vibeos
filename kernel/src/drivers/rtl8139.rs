//! Realtek RTL8139（PCI の Ethernet カード）
//!
//! カードがメモリに直接書き読みする（バスマスタ DMA）:
//! - 受信: 8KB のリング（+ 折り返さずに書き込む分の余白）。カードが [状態 16bit][長さ 16bit][フレーム][CRC]
//!   を並べて書き、こちらは読んだ位置を CAPR で知らせる（CAPR には 16 引いた値を書く。カードの癖）
//! - 送信: 4 つの送信口（TSAD/TSD）を順に使う。TSD の OWN ビットが立てば、その口はまた使える
//!
//! DMA の領域は物理ページをまとめて確保する（カーネル空間は物理と同じ番地なので、そのまま番地を渡せる）。
//! 割り込みハンドラは原因を消して net スレッドを起こすだけ。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

use vibe_net::Mac;

use super::pci;
use crate::arch::port::{inb, inl, inw, io_wait, outb, outl, outw};
use crate::arch::trap;
use crate::mm::pmm;
use crate::sync::IrqMutex;

pub const VENDOR: u16 = 0x10EC;
pub const DEVICE: u16 = 0x8139;

const IDR0: u16 = 0x00;
const TSD0: u16 = 0x10;
const TSAD0: u16 = 0x20;
const RBSTART: u16 = 0x30;
const CR: u16 = 0x37;
const CAPR: u16 = 0x38;
const IMR: u16 = 0x3C;
const ISR: u16 = 0x3E;
const TCR: u16 = 0x40;
const RCR: u16 = 0x44;
const CONFIG1: u16 = 0x52;

const CR_RST: u8 = 0x10;
const CR_RE: u8 = 0x08;
const CR_TE: u8 = 0x04;
const CR_BUFE: u8 = 0x01;

const ISR_ROK: u16 = 0x01;
const ISR_RER: u16 = 0x02;
const ISR_TOK: u16 = 0x04;
const ISR_TER: u16 = 0x08;
const ISR_RXOVW: u16 = 0x10;
const ISR_FOVW: u16 = 0x40;
const IMR_MASK: u16 = ISR_ROK | ISR_RER | ISR_TOK | ISR_TER | ISR_RXOVW | ISR_FOVW;

/// RCR: ブロードキャスト・自分宛てを受け取る、リングの終わりで折り返さずに書く（WRAP）、
/// 8KB のリング、DMA の一度の量は無制限
const RCR_VALUE: u32 = 0x08 | 0x02 | 0x80 | (7 << 8);
/// TCR: DMA の一度の量 2048 バイト
const TCR_VALUE: u32 = 7 << 8;

const TSD_OWN: u32 = 0x2000;

const RX_RING: usize = 8192;
/// リング + 先頭の 16 バイト + 折り返さずに書かれる最大のフレーム
const RX_BUF_BYTES: usize = RX_RING + 16 + 1536;
const TX_SLOTS: usize = 4;
const TX_SLOT_BYTES: usize = 2048;
const PAGE: usize = 4096;

const POLL_LIMIT: u32 = 100_000;

/// TCP で知らせる受信の窓（8KB のリングに収まる量: 1460 バイトのデータが 5 つ）
pub const RECV_WINDOW: u32 = 5 * 1460;

struct Rtl8139 {
    io: u16,
    mac: Mac,
    rx_buf: usize,
    tx_buf: usize,
    /// 次に読む受信のリングの位置
    rx_off: usize,
    /// 次に使う送信口
    tx_next: usize,
}

static CARD: IrqMutex<Option<Rtl8139>> = IrqMutex::new(None);
static EVENTS: AtomicU16 = AtomicU16::new(0);

impl Rtl8139 {
    fn in8(&self, r: u16) -> u8 {
        // SAFETY: このカードの I/O ポート（PCI の BAR0）
        unsafe { inb(self.io + r) }
    }
    fn out8(&self, r: u16, v: u8) {
        // SAFETY: 同上
        unsafe { outb(self.io + r, v) }
    }
    fn in16(&self, r: u16) -> u16 {
        // SAFETY: 同上
        unsafe { inw(self.io + r) }
    }
    fn out16(&self, r: u16, v: u16) {
        // SAFETY: 同上
        unsafe { outw(self.io + r, v) }
    }
    fn in32(&self, r: u16) -> u32 {
        // SAFETY: 同上
        unsafe { inl(self.io + r) }
    }
    fn out32(&self, r: u16, v: u32) {
        // SAFETY: 同上
        unsafe { outl(self.io + r, v) }
    }

    /// リセットして送受信を始める
    fn start(&mut self) -> bool {
        self.out8(CONFIG1, 0); // 電源を入れる
        self.out8(CR, CR_RST);
        let mut ok = false;
        for _ in 0..POLL_LIMIT {
            if self.in8(CR) & CR_RST == 0 {
                ok = true;
                break;
            }
            io_wait();
        }
        if !ok {
            return false;
        }
        self.out32(RBSTART, self.rx_buf as u32);
        for i in 0..TX_SLOTS {
            self.out32(TSAD0 + 4 * i as u16, (self.tx_buf + i * TX_SLOT_BYTES) as u32);
        }
        self.out8(CR, CR_RE | CR_TE);
        self.out32(RCR, RCR_VALUE);
        self.out32(TCR, TCR_VALUE);
        self.out16(IMR, IMR_MASK);
        self.rx_off = 0;
        self.tx_next = 0;
        true
    }

    fn rx_byte(&self, off: usize) -> u8 {
        // SAFETY: rx_buf は start で確保した RX_BUF_BYTES バイトの領域（カーネル空間で物理と同じ番地）
        unsafe { core::ptr::read_volatile((self.rx_buf + off) as *const u8) }
    }

    fn recv(&mut self) -> Recv {
        if self.in8(CR) & CR_BUFE != 0 {
            return Recv::Empty;
        }
        let off = self.rx_off;
        let status = u16::from_le_bytes([self.rx_byte(off), self.rx_byte(off + 1)]);
        let len = u16::from_le_bytes([self.rx_byte(off + 2), self.rx_byte(off + 3)]) as usize;
        if status & 0x01 == 0 || !(64..=1522).contains(&len) {
            klog!("rtl8139: bad receive header {:#06x} len {} at {}, resetting", status, len, off);
            self.start();
            return Recv::Reset;
        }
        // CRC（4 バイト）を除く
        let frame: Vec<u8> = (0..len - 4).map(|i| self.rx_byte(off + 4 + i)).collect();
        self.rx_off = (off + 4 + len + 3) & !3;
        if self.rx_off >= RX_RING {
            self.rx_off -= RX_RING;
        }
        self.out16(CAPR, (self.rx_off as u16).wrapping_sub(16));
        Recv::Frame(frame)
    }

    fn tx_idle(&self) -> bool {
        self.in32(TSD0 + 4 * self.tx_next as u16) & TSD_OWN != 0
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if frame.len() > 1514 || !self.tx_idle() {
            return false;
        }
        let slot = self.tx_buf + self.tx_next * TX_SLOT_BYTES;
        let len = frame.len().max(60);
        // SAFETY: 送信口の領域（start で確保した TX_SLOT_BYTES バイト）に書く
        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), slot as *mut u8, frame.len());
            core::ptr::write_bytes((slot + frame.len()) as *mut u8, 0, len - frame.len());
        }
        // 長さを書くと OWN が落ちて送り始める
        self.out32(TSD0 + 4 * self.tx_next as u16, len as u32);
        self.tx_next = (self.tx_next + 1) % TX_SLOTS;
        true
    }
}

enum Recv {
    Empty,
    Frame(Vec<u8>),
    Reset,
}

/// PCI の RTL8139 を初期化する。MAC アドレスと (I/O ポート, IRQ) を返す
pub fn probe(d: &pci::Device) -> Option<(Mac, u16, u8)> {
    let io = d.io_base(0)?;
    d.enable(pci::COMMAND_IO | pci::COMMAND_BUS_MASTER);
    let rx_buf = pmm::alloc_pages(RX_BUF_BYTES.div_ceil(PAGE))?;
    let tx_buf = pmm::alloc_pages((TX_SLOTS * TX_SLOT_BYTES).div_ceil(PAGE))?;
    let mut card = Rtl8139 { io, mac: Mac::ZERO, rx_buf, tx_buf, rx_off: 0, tx_next: 0 };
    let mut mac = [0u8; 6];
    for (i, m) in mac.iter_mut().enumerate() {
        *m = card.in8(IDR0 + i as u16);
    }
    card.mac = Mac(mac);
    if !card.start() {
        klog!("rtl8139: reset timed out");
        pmm::free_pages(rx_buf, RX_BUF_BYTES.div_ceil(PAGE));
        pmm::free_pages(tx_buf, (TX_SLOTS * TX_SLOT_BYTES).div_ceil(PAGE));
        return None;
    }
    let irq = d.irq;
    *CARD.lock() = Some(card);
    trap::set_irq_handler(irq, on_irq);
    Some((Mac(mac), io, irq))
}

fn on_irq() {
    let mut events = 0;
    if let Some(card) = CARD.lock().as_mut() {
        // PCI の割り込みはレベル（原因が残っている間は線が上がったまま）なので、全部消す
        for _ in 0..16 {
            let isr = card.in16(ISR) & IMR_MASK;
            if isr == 0 {
                break;
            }
            card.out16(ISR, isr);
            events |= isr;
        }
    }
    EVENTS.fetch_or(events, Ordering::AcqRel);
    crate::net::wake_net_thread();
}

pub fn has_events() -> bool {
    EVENTS.load(Ordering::Acquire) != 0
}

pub fn receive_all(mut f: impl FnMut(&[u8])) {
    let events = EVENTS.swap(0, Ordering::AcqRel);
    if events & (ISR_RXOVW | ISR_FOVW) != 0 {
        klog!("rtl8139: receive buffer overflow, resetting");
        if let Some(card) = CARD.lock().as_mut() {
            card.start();
        }
        return;
    }
    for _ in 0..64 {
        let r = match CARD.lock().as_mut() {
            Some(card) => card.recv(),
            None => return,
        };
        match r {
            Recv::Frame(frame) => f(&frame),
            Recv::Empty | Recv::Reset => return,
        }
    }
}

pub fn tx_idle() -> bool {
    CARD.lock().as_ref().is_none_or(|c| c.tx_idle())
}

pub fn send(frame: &[u8]) -> bool {
    CARD.lock().as_mut().is_some_and(|c| c.send(frame))
}

pub fn tx_timeout() {
    klog!("rtl8139: transmit timed out, resetting");
    if let Some(card) = CARD.lock().as_mut() {
        card.start();
    }
}
