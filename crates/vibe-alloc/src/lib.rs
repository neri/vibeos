//! メモリの割り当てアルゴリズム（ハードウェアに依存しない部分）
//!
//! - [`frame::FrameAllocator`]: 物理ページ（4KB）をビットマップで管理する
//! - [`heap::Heap`]: 任意の大きさのメモリを空きリスト（アドレス順・隣接ブロックは結合）で管理する

#![cfg_attr(not(test), no_std)]

pub mod frame;
pub mod heap;
