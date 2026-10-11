//! RAM ディスク: ローダがメモリに読み込んだフロッピー全体
//!
//! 書き込みはまずメモリに反映し、変更したセクタを覚えておく。`sync` でそのセクタだけを
//! フロッピーに書き戻す（書き戻すまでは電源を切ると消える）。

use alloc::vec;
use alloc::vec::Vec;

use vibe_fat::{BlockDevice, IoError, SECTOR_SIZE};

use super::fdc::{self, FdcError};

pub struct RamDisk {
    addr: usize,
    size: usize,
    /// 変更したセクタ（1 ビット 1 セクタ）
    dirty: Vec<u64>,
}

impl RamDisk {
    /// # Safety
    /// [addr, addr + size) はこの RAM ディスク専用で、カーネル空間で同じ番地に対応付けられていること
    pub unsafe fn new(addr: usize, size: usize) -> RamDisk {
        RamDisk { addr, size, dirty: vec![0; (size / SECTOR_SIZE).div_ceil(64)] }
    }

    fn is_dirty(&self, lba: usize) -> bool {
        self.dirty[lba / 64] & (1 << (lba % 64)) != 0
    }

    /// まだ書き戻していないセクタの数
    pub fn dirty_sectors(&self) -> u32 {
        self.dirty.iter().map(|w| w.count_ones()).sum()
    }

    /// 変更したセクタをフロッピーに書き戻す。書いたセクタ数を返す。
    /// 同じトラックの中で続いているセクタはまとめて書く
    pub fn sync(&mut self) -> Result<u32, FdcError> {
        let sectors = (self.size / SECTOR_SIZE).min(fdc::TOTAL_SECTORS as usize);
        let mut written = 0;
        let mut lba = 0;
        while lba < sectors {
            if !self.is_dirty(lba) {
                lba += 1;
                continue;
            }
            let track_end = (lba / 18 + 1) * 18;
            let mut end = lba + 1;
            while end < track_end.min(sectors) && self.is_dirty(end) {
                end += 1;
            }
            // SAFETY: RAM ディスクの範囲の中
            let data = unsafe {
                core::slice::from_raw_parts((self.addr + lba * SECTOR_SIZE) as *const u8, (end - lba) * SECTOR_SIZE)
            };
            fdc::write(lba as u32, data)?;
            for s in lba..end {
                self.dirty[s / 64] &= !(1 << (s % 64));
            }
            written += (end - lba) as u32;
            lba = end;
        }
        Ok(written)
    }

    fn range(&self, lba: u32, len: usize) -> Result<usize, IoError> {
        let start = lba as usize * SECTOR_SIZE;
        if len % SECTOR_SIZE != 0 || start.checked_add(len).is_none_or(|end| end > self.size) {
            return Err(IoError);
        }
        Ok(self.addr + start)
    }
}

impl BlockDevice for RamDisk {
    fn block_count(&self) -> u32 {
        (self.size / SECTOR_SIZE) as u32
    }

    fn read_blocks(&self, lba: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let src = self.range(lba, buf.len())?;
        // SAFETY: range で RAM ディスクの中に収まることを確かめた
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), buf.len()) };
        Ok(())
    }

    fn write_blocks(&mut self, lba: u32, buf: &[u8]) -> Result<(), IoError> {
        let dst = self.range(lba, buf.len())?;
        // 中身が変わらないセクタは書き戻さなくてよい（FAT は毎回全体を書くため）
        for (i, chunk) in buf.chunks(SECTOR_SIZE).enumerate() {
            let s = lba as usize + i;
            // SAFETY: range で RAM ディスクの中に収まることを確かめた
            let old = unsafe { core::slice::from_raw_parts((dst + i * SECTOR_SIZE) as *const u8, SECTOR_SIZE) };
            if old != chunk {
                self.dirty[s / 64] |= 1 << (s % 64);
            }
        }
        // SAFETY: 同上
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dst as *mut u8, buf.len()) };
        Ok(())
    }
}
