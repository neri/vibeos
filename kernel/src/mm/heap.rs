//! カーネルヒープ
//!
//! 最初に物理ページから 1MB を取り、足りなくなったら物理ページを追加で取ってくる。
//! 割り込みハンドラの中では使わないこと（DESIGN.md §6.4）。

use core::alloc::{GlobalAlloc, Layout};

use vibe_alloc::frame::PAGE_SIZE;
use vibe_alloc::heap::{GRANULE, Heap};

use super::pmm;
use crate::sync::IrqMutex;

const INITIAL_PAGES: usize = 256; // 1MB
const GROW_MIN_PAGES: usize = 64; // 256KB

struct KernelAllocator {
    heap: IrqMutex<Heap>,
}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator { heap: IrqMutex::new(Heap::empty()) };

pub fn init() {
    let addr = pmm::alloc_pages(INITIAL_PAGES).expect("heap: no memory for the initial heap");
    // SAFETY: 割り当てたばかりの物理ページ（カーネル空間では同じ番地で触れる）
    unsafe { ALLOCATOR.heap.lock().add_region(addr, INITIAL_PAGES * PAGE_SIZE) };
    klog!("heap: {} KB at {:#x}", INITIAL_PAGES * PAGE_SIZE / 1024, addr);
}

/// (空きバイト数, 管理しているバイト数, 空きブロック数)
pub fn stats() -> (usize, usize, usize) {
    let h = ALLOCATOR.heap.lock();
    (h.free_bytes(), h.total_bytes(), h.free_blocks())
}

// SAFETY: Heap の操作はすべてロックの中で行う
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.heap.lock();
        let p = heap.alloc(layout);
        if !p.is_null() {
            return p;
        }
        // 足りないので物理ページを追加する（整列のための余裕も見込む）
        let need = layout.size() + layout.align() + 2 * GRANULE;
        let pages = need.div_ceil(PAGE_SIZE).max(GROW_MIN_PAGES);
        let Some(addr) = pmm::alloc_pages(pages) else {
            return core::ptr::null_mut();
        };
        // SAFETY: 割り当てたばかりの物理ページ
        unsafe { heap.add_region(addr, pages * PAGE_SIZE) };
        heap.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc の約束により、ptr はこのアロケータが layout で返したもの
        unsafe { self.heap.lock().dealloc(ptr, layout) };
    }
}
