//! 物理ページのビットマップ管理
//!
//! ビットが 1 = 空き、0 = 使用中または存在しない。初期値（全部 0）は「何も使えない」状態で、
//! `add_free_range` で使えるメモリを登録していく。全部 0 で始まるので、静的変数に置いても
//! カーネルのイメージを大きくしない（BSS に入る）。

pub const PAGE_SIZE: usize = 4096;

/// `WORDS * 32` ページ（= `WORDS * 128KB`）までを扱う
pub struct FrameAllocator<const WORDS: usize> {
    bitmap: [u32; WORDS],
    free: usize,
    total: usize,
    /// 次に探し始める位置（ページ番号）
    hint: usize,
}

impl<const WORDS: usize> Default for FrameAllocator<WORDS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const WORDS: usize> FrameAllocator<WORDS> {
    pub const MAX_FRAMES: usize = WORDS * 32;
    /// 扱える物理アドレスの上限（これ以上は無視する）
    pub const LIMIT: usize = Self::MAX_FRAMES * PAGE_SIZE;

    pub const fn new() -> Self {
        FrameAllocator { bitmap: [0; WORDS], free: 0, total: 0, hint: 0 }
    }

    fn is_free(&self, frame: usize) -> bool {
        self.bitmap[frame / 32] & (1 << (frame % 32)) != 0
    }

    fn set_free(&mut self, frame: usize, free: bool) {
        let bit = 1 << (frame % 32);
        if free {
            self.bitmap[frame / 32] |= bit;
        } else {
            self.bitmap[frame / 32] &= !bit;
        }
    }

    /// [start, end) の中に完全に含まれるページを空きとして登録する
    pub fn add_free_range(&mut self, start: usize, end: usize) {
        let first = start.div_ceil(PAGE_SIZE);
        let last = (end.min(Self::LIMIT)) / PAGE_SIZE;
        for f in first..last {
            if !self.is_free(f) {
                self.set_free(f, true);
                self.free += 1;
                self.total += 1;
            }
        }
    }

    /// [start, end) に少しでも重なるページを使用中にする（空きから外す）
    pub fn reserve_range(&mut self, start: usize, end: usize) {
        let first = start / PAGE_SIZE;
        let last = end.min(Self::LIMIT).div_ceil(PAGE_SIZE);
        for f in first..last {
            if self.is_free(f) {
                self.set_free(f, false);
                self.free -= 1;
                self.total -= 1;
            }
        }
    }

    /// 1 ページ割り当てて物理アドレスを返す
    pub fn alloc(&mut self) -> Option<usize> {
        self.alloc_contiguous(1)
    }

    /// 連続した `count` ページを割り当てて先頭の物理アドレスを返す
    pub fn alloc_contiguous(&mut self, count: usize) -> Option<usize> {
        if count == 0 || count > self.free {
            return None;
        }
        let n = Self::MAX_FRAMES;
        // hint から最後まで、だめなら先頭から hint まで探す
        for (from, to) in [(self.hint, n), (0, self.hint.min(n))] {
            if let Some(f) = self.find_run(from, to, count) {
                for i in f..f + count {
                    self.set_free(i, false);
                }
                self.free -= count;
                self.hint = f + count;
                return Some(f * PAGE_SIZE);
            }
        }
        None
    }

    fn find_run(&self, from: usize, to: usize, count: usize) -> Option<usize> {
        let mut run_start = from;
        let mut run_len = 0;
        let mut f = from;
        while f < to {
            // 32 ページまとめて使用中なら飛ばす
            if f % 32 == 0 && self.bitmap[f / 32] == 0 {
                run_len = 0;
                f += 32;
                run_start = f;
                continue;
            }
            if self.is_free(f) {
                if run_len == 0 {
                    run_start = f;
                }
                run_len += 1;
                if run_len == count {
                    return Some(run_start);
                }
            } else {
                run_len = 0;
            }
            f += 1;
        }
        None
    }

    /// `alloc` / `alloc_contiguous` で得たページを返す
    pub fn free(&mut self, addr: usize, count: usize) {
        assert!(addr % PAGE_SIZE == 0, "free: unaligned address {addr:#x}");
        let first = addr / PAGE_SIZE;
        for f in first..first + count {
            assert!(f < Self::MAX_FRAMES && !self.is_free(f), "free: page {:#x} is not allocated", f * PAGE_SIZE);
            self.set_free(f, true);
        }
        self.free += count;
        if first < self.hint {
            self.hint = first;
        }
    }

    pub fn free_pages(&self) -> usize {
        self.free
    }

    pub fn total_pages(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Fa = FrameAllocator<4>; // 128 ページ = 512KB

    #[test]
    fn starts_empty() {
        let mut fa = Fa::new();
        assert_eq!(fa.free_pages(), 0);
        assert_eq!(fa.alloc(), None);
    }

    #[test]
    fn ranges_are_page_aligned_inward() {
        let mut fa = Fa::new();
        fa.add_free_range(100, 3 * PAGE_SIZE + 5); // ページ 1, 2 だけが完全に含まれる
        assert_eq!(fa.free_pages(), 2);
        assert_eq!(fa.alloc(), Some(PAGE_SIZE));
        assert_eq!(fa.alloc(), Some(2 * PAGE_SIZE));
        assert_eq!(fa.alloc(), None);
    }

    #[test]
    fn reserve_removes_overlapping_pages() {
        let mut fa = Fa::new();
        fa.add_free_range(0, 10 * PAGE_SIZE);
        fa.reserve_range(2 * PAGE_SIZE + 1, 4 * PAGE_SIZE); // ページ 2, 3
        assert_eq!(fa.free_pages(), 8);
        assert_eq!(fa.total_pages(), 8);
        let got: Vec<_> = (0..8).map(|_| fa.alloc().unwrap() / PAGE_SIZE).collect();
        assert_eq!(got, [0, 1, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn contiguous_skips_holes_and_wraps() {
        let mut fa = Fa::new();
        fa.add_free_range(0, 64 * PAGE_SIZE);
        fa.reserve_range(3 * PAGE_SIZE, 4 * PAGE_SIZE);
        assert_eq!(fa.alloc_contiguous(4), Some(4 * PAGE_SIZE)); // 0〜2 は 3 ページしかない
        assert_eq!(fa.alloc_contiguous(3), Some(8 * PAGE_SIZE));
        assert_eq!(fa.alloc_contiguous(53), Some(11 * PAGE_SIZE)); // 11〜63
        // 探し始めの位置より後ろにはもう無いので、先頭に戻って見つける
        assert_eq!(fa.alloc_contiguous(3), Some(0));
        assert_eq!(fa.alloc(), None);
    }

    #[test]
    fn free_restores_count_and_detects_double_free() {
        let mut fa = Fa::new();
        fa.add_free_range(0, 16 * PAGE_SIZE);
        let a = fa.alloc_contiguous(5).unwrap();
        assert_eq!(fa.free_pages(), 11);
        fa.free(a, 5);
        assert_eq!(fa.free_pages(), 16);
        let r = std::panic::catch_unwind(move || {
            let mut fa = fa;
            fa.free(a, 1)
        });
        assert!(r.is_err());
    }

    #[test]
    fn addresses_above_limit_are_ignored() {
        let mut fa = Fa::new();
        fa.add_free_range(Fa::LIMIT - PAGE_SIZE, Fa::LIMIT + 100 * PAGE_SIZE);
        assert_eq!(fa.free_pages(), 1);
    }
}
