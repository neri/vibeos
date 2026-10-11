//! メモリ管理
//!
//! - `pmm`: 物理ページ（4KB）の割り当て
//! - `heap`: カーネルヒープ（`alloc` クレートの Box / Vec / String などが使える）
//! - `paging`: ページング（カーネル空間は物理アドレスと同じ番地に対応付ける）
//!
//! ロックを取る順番: HEAP → FRAMES（ヒープが足りなくなると物理ページを取りに行く）

pub mod heap;
pub mod paging;
pub mod pmm;
pub mod space;

use crate::bootinfo::BootInfo;

pub fn init(info: &BootInfo) {
    pmm::init(info);
    paging::init();
    crate::arch::gdt::init_double_fault(paging::kernel_page_directory() as u32);
    heap::init();
}
