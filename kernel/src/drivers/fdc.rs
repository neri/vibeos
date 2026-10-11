//! フロッピーディスクコントローラ（82077AA 互換）と ISA DMA（チャネル 2）
//!
//! 起動時はローダがフロッピー全体を RAM ディスクに読み込むので、このドライバは主に
//! RAM ディスクの変更を書き戻す（sync）ために使う。1.44MB（80 シリンダ × 2 ヘッド × 18 セクタ）のみ対応。
//!
//! 実機を想定し、待ちにはすべて上限を付け、失敗したら再較正してやり直す。
//! 割り込み（IRQ 6）は「来た」という印を付けるだけで、待つ側はタイマの刻みごとに印を見る。

use core::sync::atomic::{AtomicBool, Ordering};

use super::pit;
use crate::arch::port::{inb, io_wait, outb};
use crate::arch::trap;
use crate::sync::{BootCell, Mutex};

const DOR: u16 = 0x3F2;
const MSR: u16 = 0x3F4;
const FIFO: u16 = 0x3F5;
const CCR: u16 = 0x3F7;

const DOR_NOT_RESET: u8 = 0x04;
const DOR_DMA_IRQ: u8 = 0x08;
const DOR_MOTOR_A: u8 = 0x10;

const MSR_RQM: u8 = 0x80;
const MSR_DIO: u8 = 0x40;

const CMD_SPECIFY: u8 = 0x03;
const CMD_WRITE: u8 = 0x45; // MFM | WRITE DATA
const CMD_READ: u8 = 0x46; // MFM | READ DATA
const CMD_RECALIBRATE: u8 = 0x07;
const CMD_SENSE_INTERRUPT: u8 = 0x08;
const CMD_SEEK: u8 = 0x0F;

pub const SECTOR_SIZE: usize = 512;
const SECTORS_PER_TRACK: u32 = 18;
const HEADS: u32 = 2;
const CYLINDERS: u32 = 80;
pub const TOTAL_SECTORS: u32 = SECTORS_PER_TRACK * HEADS * CYLINDERS;
const GAP3: u8 = 0x1B;
const RETRIES: u32 = 3;

/// DMA の転送領域（1 トラック分）。物理 16MB 未満で 64KB の境界をまたがないこと:
/// 16KB 境界に置けば 9KB はどこでも 64KB 境界をまたがない。カーネルは 1MB〜4MB にある
#[repr(C, align(16384))]
struct DmaBuffer([u8; SECTORS_PER_TRACK as usize * SECTOR_SIZE]);
static DMA_BUFFER: BootCell<DmaBuffer> = BootCell::new(DmaBuffer([0; SECTORS_PER_TRACK as usize * SECTOR_SIZE]));

static IRQ_RECEIVED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdcError {
    NoDrive,
    Timeout,
    /// コントローラが失敗を返した（ST0〜ST2）
    Failed(u8, u8, u8),
    OutOfRange,
}

struct Fdc {
    drive: u8,
}

/// ドライブ（無ければ None）。操作中は待つことがあるので眠る Mutex
static FDC: Mutex<Option<Fdc>> = Mutex::new(None);

/// CMOS で 1.44MB のドライブ A があるか確かめ、コントローラを初期化する（割り込み許可後に呼ぶ）
pub fn init() {
    // CMOS 0x10 の上位 4 ビットがドライブ A の種類（4 = 3.5" 1.44MB）
    // SAFETY: CMOS の読み出し（NMI 禁止ビットは立てない）
    let kind = unsafe {
        outb(0x70, 0x10);
        inb(0x71) >> 4
    };
    if kind != 4 {
        klog!("fdc: no 1.44MB drive A (CMOS type {})", kind);
        return;
    }
    trap::set_irq_handler(6, || IRQ_RECEIVED.store(true, Ordering::Release));
    let fdc = Fdc { drive: 0 };
    match fdc.reset() {
        Ok(()) => {
            klog!("fdc: drive A ready");
            *FDC.lock() = Some(fdc);
        }
        Err(e) => klog!("fdc: reset failed: {:?}", e),
    }
}

pub fn present() -> bool {
    FDC.lock().is_some()
}

/// `lba` から `buf.len() / 512` セクタ読む
#[allow(dead_code)] // 起動時はローダが BIOS で読むので、今はテストで使うだけ
pub fn read(lba: u32, buf: &mut [u8]) -> Result<(), FdcError> {
    let guard = FDC.lock();
    let fdc = guard.as_ref().ok_or(FdcError::NoDrive)?;
    fdc.motor(true);
    let r = fdc.transfer(lba, buf.len() / SECTOR_SIZE, |off, n, dma| buf[off..off + n].copy_from_slice(&dma[..n]), None);
    fdc.motor(false);
    r
}

/// `lba` から `data.len() / 512` セクタ書く
pub fn write(lba: u32, data: &[u8]) -> Result<(), FdcError> {
    let guard = FDC.lock();
    let fdc = guard.as_ref().ok_or(FdcError::NoDrive)?;
    fdc.motor(true);
    let r = fdc.transfer(lba, data.len() / SECTOR_SIZE, |_, _, _| {}, Some(data));
    fdc.motor(false);
    r
}

fn wait_irq(timeout_ms: u32) -> bool {
    let start = pit::ticks();
    let limit = (timeout_ms * pit::TICK_HZ).div_ceil(1000) + 1;
    loop {
        if IRQ_RECEIVED.swap(false, Ordering::Acquire) {
            return true;
        }
        if pit::ticks().wrapping_sub(start) > limit {
            return false;
        }
        pit::sleep_ms(1);
    }
}

fn send(byte: u8) -> Result<(), FdcError> {
    for _ in 0..100_000 {
        // SAFETY: 状態を読むだけ
        if unsafe { inb(MSR) } & (MSR_RQM | MSR_DIO) == MSR_RQM {
            // SAFETY: コントローラがコマンドを受け取れる状態
            unsafe { outb(FIFO, byte) };
            return Ok(());
        }
        io_wait();
    }
    Err(FdcError::Timeout)
}

fn receive() -> Result<u8, FdcError> {
    for _ in 0..100_000 {
        // SAFETY: 状態を読むだけ
        if unsafe { inb(MSR) } & (MSR_RQM | MSR_DIO) == (MSR_RQM | MSR_DIO) {
            // SAFETY: コントローラが結果を出している
            return Ok(unsafe { inb(FIFO) });
        }
        io_wait();
    }
    Err(FdcError::Timeout)
}

fn sense_interrupt() -> Result<(u8, u8), FdcError> {
    send(CMD_SENSE_INTERRUPT)?;
    Ok((receive()?, receive()?))
}

impl Fdc {
    fn reset(&self) -> Result<(), FdcError> {
        IRQ_RECEIVED.store(false, Ordering::Release);
        // SAFETY: DOR のリセットを下げて上げる
        unsafe {
            outb(DOR, 0);
            for _ in 0..100 {
                io_wait();
            }
            outb(DOR, DOR_NOT_RESET | DOR_DMA_IRQ);
        }
        if !wait_irq(500) {
            return Err(FdcError::Timeout);
        }
        for _ in 0..4 {
            sense_interrupt()?;
        }
        // SAFETY: 500kbps（1.44MB）
        unsafe { outb(CCR, 0) };
        // 移動 3ms, ヘッドを上げるまで 240ms, ヘッドを下ろすまで 4ms, DMA を使う
        send(CMD_SPECIFY)?;
        send(0xDF)?;
        send(0x02)?;
        // 再較正はモーターが回っていないと失敗する（実機と Bochs。QEMU は通ってしまう）
        self.motor(true);
        let r = self.recalibrate();
        self.motor(false);
        r
    }

    fn motor(&self, on: bool) {
        let dor = DOR_NOT_RESET | DOR_DMA_IRQ | self.drive | if on { DOR_MOTOR_A << self.drive } else { 0 };
        // SAFETY: モーターの制御
        unsafe { outb(DOR, dor) };
        if on {
            pit::sleep_ms(300); // 回転が安定するまで待つ
        }
    }

    fn recalibrate(&self) -> Result<(), FdcError> {
        let mut last = (0, 0);
        for _ in 0..2 {
            IRQ_RECEIVED.store(false, Ordering::Release);
            send(CMD_RECALIBRATE)?;
            send(self.drive)?;
            if !wait_irq(3000) {
                return Err(FdcError::Timeout);
            }
            let (st0, cyl) = sense_interrupt()?;
            if st0 & 0xC0 == 0 && cyl == 0 {
                return Ok(());
            }
            last = (st0, cyl);
        }
        // ST0 とシリンダを返す（原因を調べるため）
        Err(FdcError::Failed(last.0, last.1, 0))
    }

    fn seek(&self, cyl: u8, head: u8) -> Result<(), FdcError> {
        IRQ_RECEIVED.store(false, Ordering::Release);
        send(CMD_SEEK)?;
        send(head << 2 | self.drive)?;
        send(cyl)?;
        if !wait_irq(3000) {
            return Err(FdcError::Timeout);
        }
        let (st0, pcn) = sense_interrupt()?;
        if st0 & 0xC0 != 0 || pcn != cyl {
            return Err(FdcError::Failed(st0, 0, 0));
        }
        pit::sleep_ms(15); // ヘッドが落ち着くまで
        Ok(())
    }

    /// トラックの区切りごとに読み書きする。読んだときは `on_read(オフセット, バイト数, DMA の中身)` を呼ぶ
    fn transfer(
        &self,
        lba: u32,
        count: usize,
        mut on_read: impl FnMut(usize, usize, &[u8]),
        write: Option<&[u8]>,
    ) -> Result<(), FdcError> {
        if lba as usize + count > TOTAL_SECTORS as usize {
            return Err(FdcError::OutOfRange);
        }
        let mut done = 0;
        while done < count {
            let cur = lba + done as u32;
            let sector = cur % SECTORS_PER_TRACK;
            let n = ((SECTORS_PER_TRACK - sector) as usize).min(count - done);
            let bytes = n * SECTOR_SIZE;
            let off = done * SECTOR_SIZE;
            let mut last = Err(FdcError::Timeout);
            for attempt in 0..RETRIES {
                if attempt > 0 {
                    let _ = self.recalibrate();
                }
                if let Some(data) = write {
                    // SAFETY: DMA バッファはこのドライバ（FDC の Mutex を持っている間）だけが使う
                    let dma = unsafe { &mut (*DMA_BUFFER.as_ptr()).0 };
                    dma[..bytes].copy_from_slice(&data[off..off + bytes]);
                }
                last = self.transfer_track(cur, n, write.is_some());
                if last.is_ok() {
                    break;
                }
            }
            last?;
            if write.is_none() {
                // SAFETY: 同上
                let dma = unsafe { &(*DMA_BUFFER.as_ptr()).0 };
                on_read(off, bytes, dma);
            }
            done += n;
        }
        Ok(())
    }

    /// 同じトラックの中の `n` セクタを 1 回のコマンドで読み書きする
    fn transfer_track(&self, lba: u32, n: usize, write: bool) -> Result<(), FdcError> {
        let cyl = (lba / (SECTORS_PER_TRACK * HEADS)) as u8;
        let head = ((lba / SECTORS_PER_TRACK) % HEADS) as u8;
        let sector = (lba % SECTORS_PER_TRACK + 1) as u8;
        self.seek(cyl, head)?;
        setup_dma(n * SECTOR_SIZE, write);
        IRQ_RECEIVED.store(false, Ordering::Release);
        send(if write { CMD_WRITE } else { CMD_READ })?;
        send(head << 2 | self.drive)?;
        send(cyl)?;
        send(head)?;
        send(sector)?;
        send(2)?; // 512 バイト
        send(sector + n as u8 - 1)?; // このトラックで最後に扱うセクタ
        send(GAP3)?;
        send(0xFF)?;
        if !wait_irq(3000) {
            return Err(FdcError::Timeout);
        }
        let mut res = [0u8; 7];
        for r in &mut res {
            *r = receive()?;
        }
        let (st0, st1, st2) = (res[0], res[1], res[2]);
        if st0 & 0xC0 != 0 {
            return Err(FdcError::Failed(st0, st1, st2));
        }
        Ok(())
    }
}

/// ISA DMA のチャネル 2 を設定する（`write` ならメモリ → ディスク）
fn setup_dma(len: usize, write: bool) {
    let addr = DMA_BUFFER.as_ptr() as usize;
    let count = len - 1;
    let mode = if write { 0x4A } else { 0x46 }; // 単一転送, アドレス増加, チャネル 2
    // SAFETY: DMA コントローラの標準的な設定手順。バッファは 16MB 未満・64KB 境界をまたがない
    unsafe {
        outb(0x0A, 0x06); // チャネル 2 を止める
        outb(0x0C, 0xFF); // 下位/上位の切り替えを初期化
        outb(0x04, addr as u8);
        outb(0x04, (addr >> 8) as u8);
        outb(0x81, (addr >> 16) as u8); // ページ
        outb(0x0C, 0xFF);
        outb(0x05, count as u8);
        outb(0x05, (count >> 8) as u8);
        outb(0x0B, mode);
        outb(0x0A, 0x02); // チャネル 2 を動かす
    }
}
