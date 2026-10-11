//! 物理ページの割り当て
//!
//! 1MB〜1GB の使用可能領域（E820）を管理する。1MB 未満（BIOS の領域・ブート情報・VRAM など）と
//! 1GB 以上は扱わない。カーネル本体と RAM ディスクは予約済みにする。

use vibe_alloc::frame::{FrameAllocator, PAGE_SIZE};

use crate::bootinfo::{BootInfo, E820_USABLE};
use crate::sync::IrqMutex;

const BITMAP_WORDS: usize = 8192; // 8192 * 32 ページ = 1GB
type Frames = FrameAllocator<BITMAP_WORDS>;

/// 管理する物理アドレスの上限
pub const LIMIT: usize = Frames::LIMIT;
const LOW_MEMORY_END: usize = 0x10_0000;

static FRAMES: IrqMutex<Frames> = IrqMutex::new(Frames::new());

/// 使用可能な RAM の最後（ページングで対応付ける範囲の上限）
static RAM_END: IrqMutex<usize> = IrqMutex::new(0);

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

pub fn init(info: &BootInfo) {
    let mut frames = FRAMES.lock();
    let mut ram_end = 0;
    for e in info.memory_map() {
        if e.kind != E820_USABLE || e.base >= LIMIT as u64 {
            continue;
        }
        let start = (e.base as usize).max(LOW_MEMORY_END);
        let end = (e.base + e.len).min(LIMIT as u64) as usize;
        if start < end {
            frames.add_free_range(start, end);
            ram_end = ram_end.max(end);
        }
    }
    // リンカスクリプトで定義した番地を取るだけ（中身は読まない）
    let (kstart, kend) = (&raw const __kernel_start as usize, &raw const __kernel_end as usize);
    frames.reserve_range(kstart, kend);
    let rd = info.ramdisk_addr as usize;
    frames.reserve_range(rd, rd + info.ramdisk_size as usize);
    *RAM_END.lock() = ram_end;
    klog!(
        "pmm: {} KB free of {} KB, kernel {:#x}-{:#x}, ram end {:#x}",
        frames.free_pages() * 4,
        frames.total_pages() * 4,
        kstart,
        kend,
        ram_end
    );
}

pub fn ram_end() -> usize {
    *RAM_END.lock()
}

/// 1 ページ割り当てる（中身は不定）
pub fn alloc_page() -> Option<usize> {
    FRAMES.lock().alloc()
}

/// 連続した `count` ページを割り当てる（中身は不定）
pub fn alloc_pages(count: usize) -> Option<usize> {
    FRAMES.lock().alloc_contiguous(count)
}

/// 1 ページ割り当てて 0 で埋める
pub fn alloc_zeroed_page() -> Option<usize> {
    let addr = alloc_page()?;
    // SAFETY: 割り当てたばかりのページで、カーネル空間では物理アドレスと同じ番地で触れる
    unsafe { core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE) };
    Some(addr)
}

pub fn free_pages(addr: usize, count: usize) {
    FRAMES.lock().free(addr, count);
}

/// (空きページ数, 管理しているページ数)
pub fn stats() -> (usize, usize) {
    let f = FRAMES.lock();
    (f.free_pages(), f.total_pages())
}
