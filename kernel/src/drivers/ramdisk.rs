//! RAM ディスク: ローダがメモリに読み込んだフロッピー全体
//!
//! 書き込みはメモリ上にだけ反映される（電源を切ると消える）。

use vibe_fat::{BlockDevice, IoError, SECTOR_SIZE};

pub struct RamDisk {
    addr: usize,
    size: usize,
}

impl RamDisk {
    /// # Safety
    /// [addr, addr + size) はこの RAM ディスク専用で、カーネル空間で同じ番地に対応付けられていること
    pub unsafe fn new(addr: usize, size: usize) -> RamDisk {
        RamDisk { addr, size }
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
        // SAFETY: 同上
        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), dst as *mut u8, buf.len()) };
        Ok(())
    }
}
