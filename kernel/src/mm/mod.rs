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

use crate::bootinfo::BootInfo;

pub fn init(info: &BootInfo) {
    pmm::init(info);
    paging::init();
    heap::init();
}
