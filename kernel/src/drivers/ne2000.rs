//! NE2000 互換の Ethernet カード（ISA, National Semiconductor DP8390）
//!
//! カードの 16KB のメモリ（ページ 0x40〜0x7F, 1 ページ 256 バイト）を、送信用 6 ページと
//! 受信のリング（0x46〜0x7F）に分けて使う。CPU との間のデータは「リモート DMA」（データポートを
//! 16bit ずつ読み書きする PIO）でやり取りする。
//!
//! - 割り込みハンドラは割り込みの原因（ISR）を受け取って消し、印を付けて net スレッドを起こすだけ
//! - 受信のリングから取り出すのも送信も net スレッドが行う。レジスタのページ切り替えと割り込みが
//!   混ざらないよう、カードに触る間はロック（割り込み禁止）を持つ
//! - 実機を想定し、待ちにはすべて上限を付ける。リングが壊れた・あふれたときはカードを初期化し直す

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use vibe_net::Mac;

use crate::arch::port::{inb, inw, io_wait, outb, outw};
use crate::arch::trap;
use crate::sync::IrqMutex;

// ページ 0 のレジスタ
const CR: u16 = 0x00;
const PSTART: u16 = 0x01;
const PSTOP: u16 = 0x02;
const BNRY: u16 = 0x03;
const TPSR: u16 = 0x04;
const TBCR0: u16 = 0x05;
const TBCR1: u16 = 0x06;
const ISR: u16 = 0x07;
const RSAR0: u16 = 0x08;
const RSAR1: u16 = 0x09;
const RBCR0: u16 = 0x0A;
const RBCR1: u16 = 0x0B;
const RCR: u16 = 0x0C;
const TCR: u16 = 0x0D;
const DCR: u16 = 0x0E;
const IMR: u16 = 0x0F;
// ページ 1 のレジスタ
const PAR0: u16 = 0x01;
const CURR: u16 = 0x07;
const MAR0: u16 = 0x08;
const DATA: u16 = 0x10;
const RESET: u16 = 0x1F;

// CR のビット
const CR_STP: u8 = 0x01;
const CR_STA: u8 = 0x02;
const CR_TXP: u8 = 0x04;
const CR_RD_READ: u8 = 0x08;
const CR_RD_WRITE: u8 = 0x10;
const CR_RD_ABORT: u8 = 0x20;
const CR_PAGE1: u8 = 0x40;

// ISR のビット
const ISR_PRX: u8 = 0x01;
const ISR_PTX: u8 = 0x02;
const ISR_RXE: u8 = 0x04;
const ISR_TXE: u8 = 0x08;
const ISR_OVW: u8 = 0x10;
const ISR_RDC: u8 = 0x40;
const ISR_RST: u8 = 0x80;

/// 割り込みを受ける原因: 受信・送信・それぞれの失敗・リングのあふれ
const IMR_MASK: u8 = ISR_PRX | ISR_PTX | ISR_RXE | ISR_TXE | ISR_OVW;
/// DCR: 16bit 転送・リトルエンディアン・通常動作・FIFO 8 バイト
const DCR_WORD: u8 = 0x49;
/// RCR: ブロードキャストを受け取る
const RCR_BROADCAST: u8 = 0x04;
/// RCR: 受け取らない（初期化中）
const RCR_MONITOR: u8 = 0x20;
/// TCR: 内部ループバック（初期化中は外に出さない）
const TCR_LOOPBACK: u8 = 0x02;

const TX_START: u8 = 0x40;
const RX_START: u8 = 0x46;
const RX_STOP: u8 = 0x80;

/// TCP で知らせる受信の窓（受信のリングに余裕を持って収まる量: 1460 バイトのデータが 5 つ）
pub const RECV_WINDOW: u32 = 5 * 1460;

/// 受信のヘッダの長さに入る最短・最長（ヘッダ 4 バイト + フレーム 60〜1514 + 実機では CRC 4 バイト）
const MIN_COUNT: usize = 4 + 60;
const MAX_COUNT: usize = 4 + 1518;

/// レジスタの応答を待つ回数の上限（io_wait 1 回は約 1µs）
const POLL_LIMIT: u32 = 100_000;

pub struct Ne2000 {
    io: u16,
    mac: Mac,
    /// 次に読む受信のページ
    next_page: u8,
}

/// カード（無ければ None）。触る間は割り込みを止める
static CARD: IrqMutex<Option<Ne2000>> = IrqMutex::new(None);
/// 割り込みで受け取った原因（net スレッドが取り出す）
static EVENTS: AtomicU8 = AtomicU8::new(0);
/// 送信中（送り終わりの割り込みで落とす）
static TX_BUSY: AtomicBool = AtomicBool::new(false);

impl Ne2000 {
    fn reg(&self, r: u16) -> u8 {
        // SAFETY: このカードの I/O ポート（probe で確かめた）
        unsafe { inb(self.io + r) }
    }

    fn set(&self, r: u16, v: u8) {
        // SAFETY: 同上
        unsafe { outb(self.io + r, v) }
    }

    /// リモート DMA の終わり（ISR.RDC）を待って消す
    fn wait_rdc(&self) -> bool {
        for _ in 0..POLL_LIMIT {
            if self.reg(ISR) & ISR_RDC != 0 {
                self.set(ISR, ISR_RDC);
                return true;
            }
            io_wait();
        }
        false
    }

    /// カードのメモリ `addr` から `buf` に読む（長さは偶数に切り上げて転送する）
    fn remote_read(&self, addr: u16, buf: &mut [u8]) {
        let len = buf.len().next_multiple_of(2);
        self.set(CR, CR_RD_ABORT | CR_STA);
        self.set(RBCR0, len as u8);
        self.set(RBCR1, (len >> 8) as u8);
        self.set(RSAR0, addr as u8);
        self.set(RSAR1, (addr >> 8) as u8);
        self.set(CR, CR_RD_READ | CR_STA);
        for i in (0..len).step_by(2) {
            // SAFETY: データポートを 16bit で読む（DCR で 16bit 転送にしてある）
            let w = unsafe { inw(self.io + DATA) }.to_le_bytes();
            buf[i] = w[0];
            if i + 1 < buf.len() {
                buf[i + 1] = w[1];
            }
        }
        self.wait_rdc();
    }

    fn remote_write(&self, addr: u16, data: &[u8]) -> bool {
        let len = data.len().next_multiple_of(2);
        self.set(CR, CR_RD_ABORT | CR_STA);
        self.set(ISR, ISR_RDC);
        self.set(RBCR0, len as u8);
        self.set(RBCR1, (len >> 8) as u8);
        self.set(RSAR0, addr as u8);
        self.set(RSAR1, (addr >> 8) as u8);
        self.set(CR, CR_RD_WRITE | CR_STA);
        for i in (0..len).step_by(2) {
            let w = u16::from_le_bytes([data[i], data.get(i + 1).copied().unwrap_or(0)]);
            // SAFETY: データポートに 16bit で書く
            unsafe { outw(self.io + DATA, w) };
        }
        self.wait_rdc()
    }

    /// レジスタを初期化して動かし始める（リセットの後・リングが壊れたとき）
    fn start(&mut self) {
        self.set(CR, CR_RD_ABORT | CR_STP);
        self.set(DCR, DCR_WORD);
        self.set(RBCR0, 0);
        self.set(RBCR1, 0);
        self.set(RCR, RCR_MONITOR);
        self.set(TCR, TCR_LOOPBACK);
        self.set(PSTART, RX_START);
        self.set(PSTOP, RX_STOP);
        self.set(BNRY, RX_START);
        self.set(TPSR, TX_START);
        self.set(ISR, 0xFF);
        self.set(IMR, 0);
        // ページ 1: 自分の MAC, マルチキャストは受けない, 書き込み位置
        self.set(CR, CR_PAGE1 | CR_RD_ABORT | CR_STP);
        for (i, b) in self.mac.0.iter().enumerate() {
            self.set(PAR0 + i as u16, *b);
        }
        for i in 0..8 {
            self.set(MAR0 + i, 0);
        }
        self.set(CURR, RX_START + 1);
        self.next_page = RX_START + 1;
        self.set(CR, CR_RD_ABORT | CR_STA);
        self.set(TCR, 0);
        self.set(RCR, RCR_BROADCAST);
        self.set(ISR, 0xFF);
        self.set(IMR, IMR_MASK);
        TX_BUSY.store(false, Ordering::Release);
    }

    /// 受信のリングから 1 つ取り出す
    fn recv(&mut self) -> Recv {
        self.set(CR, CR_PAGE1 | CR_RD_ABORT | CR_STA);
        let curr = self.reg(CURR);
        self.set(CR, CR_RD_ABORT | CR_STA);
        if curr == self.next_page {
            return Recv::Empty;
        }
        let mut hdr = [0u8; 4];
        self.remote_read((self.next_page as u16) << 8, &mut hdr);
        let (status, next) = (hdr[0], hdr[1]);
        let count = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
        if !(RX_START..RX_STOP).contains(&next) || !(MIN_COUNT..=MAX_COUNT).contains(&count) {
            klog!("ne2000: bad receive header {:02x?} at page {:#x}, resetting", hdr, self.next_page);
            self.start();
            return Recv::Reset;
        }
        let len = count - 4;
        let mut frame = alloc::vec![0u8; len];
        let start = ((self.next_page as usize) << 8) + 4;
        let ring_end = (RX_STOP as usize) << 8;
        if start + len > ring_end {
            // リングの終わりで折り返している
            let first = ring_end - start;
            self.remote_read(start as u16, &mut frame[..first]);
            self.remote_read((RX_START as u16) << 8, &mut frame[first..]);
        } else {
            self.remote_read(start as u16, &mut frame);
        }
        self.next_page = next;
        // BNRY は「読み終わったページ」（次に読むページの 1 つ手前）
        self.set(BNRY, if next == RX_START { RX_STOP - 1 } else { next - 1 });
        if status & 0x01 == 0 {
            return Recv::Bad; // 受信エラー（CRC など）
        }
        Recv::Frame(frame)
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        let len = frame.len().max(60);
        let mut buf = [0u8; 1536];
        buf[..frame.len()].copy_from_slice(frame);
        if !self.remote_write((TX_START as u16) << 8, &buf[..len]) {
            klog!("ne2000: remote DMA write timed out");
            self.start();
            return false;
        }
        self.set(TPSR, TX_START);
        self.set(TBCR0, len as u8);
        self.set(TBCR1, (len >> 8) as u8);
        TX_BUSY.store(true, Ordering::Release);
        self.set(CR, CR_RD_ABORT | CR_TXP | CR_STA);
        true
    }
}

enum Recv {
    Empty,
    Frame(Vec<u8>),
    Bad,
    Reset,
}

/// I/O ポート `io` に NE2000 があるか確かめ、あれば初期化して MAC アドレスを返す
pub fn probe(io: u16, irq: u8) -> Option<Mac> {
    let mut card = Ne2000 { io, mac: Mac::ZERO, next_page: RX_START + 1 };
    // 何もつながっていないポートは 0xFF を返す
    if card.reg(CR) == 0xFF {
        return None;
    }
    // リセット: リセットポートを読んで書く → ISR.RST が立つのを待つ
    // SAFETY: NE2000 のリセットポート
    unsafe { outb(io + RESET, inb(io + RESET)) };
    let mut ok = false;
    for _ in 0..POLL_LIMIT {
        if card.reg(ISR) & ISR_RST != 0 {
            ok = true;
            break;
        }
        io_wait();
    }
    if !ok {
        return None;
    }
    card.set(ISR, 0xFF);
    card.set(CR, CR_RD_ABORT | CR_STP);
    card.set(DCR, DCR_WORD);
    card.set(RBCR0, 0);
    card.set(RBCR1, 0);
    card.set(IMR, 0);
    // アドレスの PROM（16bit 転送では 1 バイトずつ 2 倍に並ぶ。14・15 番目は 'W' = NE2000 の印）
    let mut prom = [0u8; 32];
    card.remote_read(0, &mut prom);
    let mut mac = [0u8; 6];
    for (i, m) in mac.iter_mut().enumerate() {
        *m = prom[i * 2];
    }
    card.mac = Mac(mac);
    if card.mac == Mac::ZERO || card.mac.is_multicast() {
        klog!("ne2000: bad MAC address {} at {:#x}", card.mac, io);
        return None;
    }
    if prom[28] != 0x57 || prom[30] != 0x57 {
        klog!("ne2000: no NE2000 signature at {:#x} (continuing)", io);
    }
    card.start();
    *CARD.lock() = Some(card);
    trap::set_irq_handler(irq, on_irq);
    Some(Mac(mac))
}

fn on_irq() {
    let mut events = 0;
    if let Some(card) = CARD.lock().as_mut() {
        // 原因を消すまで割り込みの線が下がらない（エッジで受けているので、消し残すと次が来ない）。
        // 消している間に次の原因が立つことがあるので、無くなるまで繰り返す
        for _ in 0..16 {
            let isr = card.reg(ISR) & IMR_MASK;
            if isr == 0 {
                break;
            }
            card.set(ISR, isr);
            events |= isr;
        }
    }
    if events & (ISR_PTX | ISR_TXE) != 0 {
        TX_BUSY.store(false, Ordering::Release);
    }
    EVENTS.fetch_or(events, Ordering::AcqRel);
    crate::net::wake_net_thread();
}

/// 割り込みの原因が届いているか（net スレッドの待ちの条件）
pub fn has_events() -> bool {
    EVENTS.load(Ordering::Acquire) != 0
}

/// 届いているフレームをすべて取り出して `f` に渡す
pub fn receive_all(mut f: impl FnMut(&[u8])) {
    let events = EVENTS.swap(0, Ordering::AcqRel);
    if events & ISR_OVW != 0 {
        // リングがあふれた: 簡単のため、溜まっていたものは捨てて初期化し直す
        klog!("ne2000: receive ring overflow, resetting");
        if let Some(card) = CARD.lock().as_mut() {
            card.start();
        }
        return;
    }
    // 割り込みを取りこぼしても（実機）ここで毎回リングを見るので、受け取りは止まらない
    for _ in 0..64 {
        let r = match CARD.lock().as_mut() {
            Some(card) => card.recv(),
            None => return,
        };
        match r {
            Recv::Frame(frame) => f(&frame),
            Recv::Bad => {}
            Recv::Empty | Recv::Reset => return,
        }
    }
}

/// 送信中でない
pub fn tx_idle() -> bool {
    !TX_BUSY.load(Ordering::Acquire)
}

/// 前の送信が終わっていること（tx_idle）を確かめてから呼ぶ
pub fn send(frame: &[u8]) -> bool {
    if frame.len() > 1514 {
        return false;
    }
    match CARD.lock().as_mut() {
        Some(card) => card.send(frame),
        None => false,
    }
}

/// 送り終わりの割り込みが来ないとき（実機の不調）: 送信中の印を落としてカードを初期化し直す
pub fn tx_timeout() {
    klog!("ne2000: transmit timed out, resetting");
    if let Some(card) = CARD.lock().as_mut() {
        card.start();
    }
}
