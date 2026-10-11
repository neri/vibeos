//! ヒープ（空きリスト方式）
//!
//! 空きブロックをアドレス順の連結リストで持ち、先頭から最初に入るブロックを使う（first fit）。
//! 解放時は隣の空きブロックと結合する。ブロックの大きさと先頭は常に `GRANULE` の倍数なので、
//! 分割して余った部分も必ず空きブロックとして持てる（端数で失われるメモリが出ない）。

use core::alloc::Layout;
use core::mem::{align_of, size_of};
use core::ptr::{self, NonNull};

struct FreeBlock {
    size: usize,
    next: Option<NonNull<FreeBlock>>,
}

/// 割り当ての単位（空きブロックの管理情報が入る大きさ）
pub const GRANULE: usize = size_of::<FreeBlock>();
const _: () = assert!(GRANULE.is_power_of_two() && GRANULE >= align_of::<FreeBlock>());

pub struct Heap {
    head: Option<NonNull<FreeBlock>>,
    free: usize,
    total: usize,
}

// SAFETY: Heap は自分が管理するメモリへのポインタしか持たない。使う側がロックで守る
unsafe impl Send for Heap {}

impl Default for Heap {
    fn default() -> Self {
        Self::empty()
    }
}

fn align_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

/// `layout` に対して実際に確保するブロックの大きさ
fn block_size(layout: &Layout) -> usize {
    align_up(layout.size().max(1), GRANULE)
}

impl Heap {
    pub const fn empty() -> Self {
        Heap { head: None, free: 0, total: 0 }
    }

    /// 管理するメモリを追加する（複数回呼んでよい。離れた領域でもよい）
    ///
    /// # Safety
    /// [start, start + size) は書き込み可能で、他の誰も使っておらず、ずっと有効であること。
    pub unsafe fn add_region(&mut self, start: usize, size: usize) {
        let begin = align_up(start, GRANULE);
        let end = (start + size) & !(GRANULE - 1);
        if end <= begin {
            return;
        }
        self.total += end - begin;
        // SAFETY: 呼び出し側の保証どおり、この範囲は自由に使える
        unsafe { self.insert(begin, end - begin) };
    }

    pub fn free_bytes(&self) -> usize {
        self.free
    }

    pub fn total_bytes(&self) -> usize {
        self.total
    }

    /// 空きブロックの数（断片化の目安）
    pub fn free_blocks(&self) -> usize {
        let mut n = 0;
        let mut cur = self.head;
        while let Some(b) = cur {
            n += 1;
            // SAFETY: リストの各要素は有効な空きブロック
            cur = unsafe { b.as_ref().next };
        }
        n
    }

    /// 確保できなければ null を返す
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let size = block_size(&layout);
        let align = layout.align().max(GRANULE);
        let mut link: *mut Option<NonNull<FreeBlock>> = &mut self.head;
        // SAFETY: link は常に head か、リスト中の有効なブロックの next を指す
        unsafe {
            while let Some(block) = *link {
                let b = block.as_ptr() as usize;
                let b_size = (*block.as_ptr()).size;
                let start = align_up(b, align);
                let front = start - b;
                if front + size <= b_size {
                    let back = b_size - front - size;
                    let next = (*block.as_ptr()).next;
                    // このブロックをリストから外し、前後の余りを空きとして戻す
                    *link = next;
                    if back > 0 {
                        let rest = (start + size) as *mut FreeBlock;
                        rest.write(FreeBlock { size: back, next: *link });
                        *link = Some(NonNull::new_unchecked(rest));
                    }
                    if front > 0 {
                        let head = b as *mut FreeBlock;
                        head.write(FreeBlock { size: front, next: *link });
                        *link = Some(NonNull::new_unchecked(head));
                    }
                    self.free -= size;
                    return start as *mut u8;
                }
                link = &mut (*block.as_ptr()).next;
            }
        }
        ptr::null_mut()
    }

    /// # Safety
    /// `ptr` はこのヒープの `alloc(layout)` が返したもので、まだ解放していないこと。
    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let size = block_size(&layout);
        // SAFETY: 呼び出し側の保証により、この範囲は使われていない
        unsafe { self.insert(ptr as usize, size) };
    }

    /// 空きブロックをアドレス順に挿入し、前後と結合する
    unsafe fn insert(&mut self, addr: usize, size: usize) {
        let mut prev: Option<NonNull<FreeBlock>> = None;
        let mut cur = self.head;
        // SAFETY: リストの各要素は有効な空きブロック。addr..addr+size は呼び出し側が渡した空き領域
        unsafe {
            while let Some(c) = cur {
                if c.as_ptr() as usize > addr {
                    break;
                }
                prev = cur;
                cur = (*c.as_ptr()).next;
            }
            if let Some(c) = cur {
                debug_assert!(addr + size <= c.as_ptr() as usize, "heap: double free or overlap at {addr:#x}");
            }
            if let Some(p) = prev {
                debug_assert!(
                    p.as_ptr() as usize + (*p.as_ptr()).size <= addr,
                    "heap: double free or overlap at {addr:#x}"
                );
            }

            let node = addr as *mut FreeBlock;
            node.write(FreeBlock { size, next: cur });
            let mut node = NonNull::new_unchecked(node);
            match prev {
                Some(p) => (*p.as_ptr()).next = Some(node),
                None => self.head = Some(node),
            }
            self.free += size;

            // 後ろと結合
            if let Some(c) = cur {
                if addr + size == c.as_ptr() as usize {
                    (*node.as_ptr()).size += (*c.as_ptr()).size;
                    (*node.as_ptr()).next = (*c.as_ptr()).next;
                }
            }
            // 前と結合
            if let Some(p) = prev {
                if p.as_ptr() as usize + (*p.as_ptr()).size == addr {
                    (*p.as_ptr()).size += (*node.as_ptr()).size;
                    (*p.as_ptr()).next = (*node.as_ptr()).next;
                    node = p;
                }
            }
            let _ = node;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用に、整列したメモリ領域を用意してヒープに渡す
    fn arena(bytes: usize) -> (Vec<u128>, Heap) {
        let mut mem = vec![0u128; bytes / 16];
        let mut h = Heap::empty();
        unsafe { h.add_region(mem.as_mut_ptr() as usize, bytes) };
        (mem, h)
    }

    fn lay(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    #[test]
    fn alloc_and_free_restores_everything() {
        let (_m, mut h) = arena(4096);
        let a = h.alloc(lay(100, 4));
        let b = h.alloc(lay(1, 1));
        let c = h.alloc(lay(300, 8));
        assert!(!a.is_null() && !b.is_null() && !c.is_null());
        assert!(h.free_bytes() < 4096);
        unsafe {
            h.dealloc(b, lay(1, 1));
            h.dealloc(a, lay(100, 4));
            h.dealloc(c, lay(300, 8));
        }
        assert_eq!(h.free_bytes(), 4096);
        assert_eq!(h.free_blocks(), 1);
    }

    #[test]
    fn respects_large_alignment() {
        let (_m, mut h) = arena(8192);
        let _ = h.alloc(lay(8, 8));
        let p = h.alloc(lay(64, 1024));
        assert_eq!(p as usize % 1024, 0);
        unsafe { h.dealloc(p, lay(64, 1024)) };
        let q = h.alloc(lay(16, 512));
        assert_eq!(q as usize % 512, 0);
    }

    #[test]
    fn out_of_memory_returns_null() {
        let (_m, mut h) = arena(256);
        assert!(h.alloc(lay(512, 4)).is_null());
        let p = h.alloc(lay(256, 4));
        assert!(!p.is_null());
        assert!(h.alloc(lay(1, 1)).is_null());
    }

    #[test]
    fn multiple_regions() {
        let (_m1, mut h) = arena(256);
        let mut m2 = vec![0u128; 64];
        unsafe { h.add_region(m2.as_mut_ptr() as usize, 1024) };
        assert_eq!(h.total_bytes(), 1280);
        let p = h.alloc(lay(800, 8));
        assert!(!p.is_null());
    }

    #[test]
    fn random_stress_keeps_blocks_disjoint() {
        let (_m, mut h) = arena(64 * 1024);
        let mut live: Vec<(usize, Layout, u8)> = Vec::new();
        let mut seed = 12345u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for round in 0..20_000 {
            if live.len() < 50 && rnd() % 3 != 0 {
                let l = lay(1 + rnd() as usize % 700, 1 << (rnd() % 7));
                let p = h.alloc(l);
                if p.is_null() {
                    continue;
                }
                assert_eq!(p as usize % l.align(), 0);
                let tag = (round % 251) as u8;
                unsafe { ptr::write_bytes(p, tag, l.size()) };
                live.push((p as usize, l, tag));
            } else if !live.is_empty() {
                let i = rnd() as usize % live.len();
                let (p, l, tag) = live.swap_remove(i);
                // 他の割り当てに上書きされていないこと
                let s = unsafe { std::slice::from_raw_parts(p as *const u8, l.size()) };
                assert!(s.iter().all(|&b| b == tag), "block at {p:#x} was overwritten");
                unsafe { h.dealloc(p as *mut u8, l) };
            }
        }
        for (p, l, _) in live.drain(..) {
            unsafe { h.dealloc(p as *mut u8, l) };
        }
        assert_eq!(h.free_bytes(), 64 * 1024);
        assert_eq!(h.free_blocks(), 1);
    }
}
