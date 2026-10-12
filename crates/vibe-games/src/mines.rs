//! マインスイーパ
//!
//! 地雷は最初に開いたときに置く（開いたマスとその周りには置かない。最初の 1 回で負けないように）。
//! 周りに地雷の無いマスを開くと、つながった同じマスと縁の数字のマスをまとめて開く。
//! 地雷を開いたら負け（すべての地雷を見せる）、地雷でないマスをすべて開いたら勝ち。

use alloc::vec;
use alloc::vec::Vec;

use crate::Rng;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub mine: bool,
    pub open: bool,
    pub flag: bool,
    /// 周り 8 マスの地雷の数
    pub around: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// まだ 1 つも開いていない
    Ready,
    Playing,
    Won,
    /// 負けた（開いた地雷のマス）
    Lost(usize, usize),
}

pub struct Mines {
    w: usize,
    h: usize,
    mines: usize,
    cells: Vec<Cell>,
    state: State,
    flags: usize,
    opened: usize,
    rng: Rng,
}

impl Mines {
    pub fn new(w: usize, h: usize, mines: usize, seed: u32) -> Mines {
        let mines = mines.min(w * h - 9.min(w * h));
        Mines { w, h, mines, cells: vec![Cell::default(); w * h], state: State::Ready, flags: 0, opened: 0, rng: Rng::new(seed) }
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn cell(&self, x: usize, y: usize) -> Cell {
        self.cells[y * self.w + x]
    }

    /// 残りの地雷の数（地雷の数 − 旗の数。負にもなる）
    pub fn mines_left(&self) -> i32 {
        self.mines as i32 - self.flags as i32
    }

    pub fn is_over(&self) -> bool {
        matches!(self.state, State::Won | State::Lost(..))
    }

    fn neighbors(&self, x: usize, y: usize) -> impl Iterator<Item = (usize, usize)> + use<> {
        let (w, h) = (self.w as i32, self.h as i32);
        let (x, y) = (x as i32, y as i32);
        (-1..=1)
            .flat_map(move |dy| (-1..=1).map(move |dx| (x + dx, y + dy)))
            .filter(move |&(nx, ny)| (nx, ny) != (x, y) && nx >= 0 && ny >= 0 && nx < w && ny < h)
            .map(|(nx, ny)| (nx as usize, ny as usize))
    }

    /// 地雷を置く（(sx, sy) とその周りには置かない）
    fn place(&mut self, sx: usize, sy: usize) {
        let safe: Vec<(usize, usize)> = core::iter::once((sx, sy)).chain(self.neighbors(sx, sy)).collect();
        let mut placed = 0;
        while placed < self.mines {
            let i = self.rng.below((self.w * self.h) as u32) as usize;
            let (x, y) = (i % self.w, i / self.w);
            if self.cells[i].mine || safe.contains(&(x, y)) {
                continue;
            }
            self.cells[i].mine = true;
            placed += 1;
        }
        for y in 0..self.h {
            for x in 0..self.w {
                let n = self.neighbors(x, y).filter(|&(nx, ny)| self.cell(nx, ny).mine).count();
                self.cells[y * self.w + x].around = n as u8;
            }
        }
        self.state = State::Playing;
    }

    /// マスを開く。何か変わったら true
    pub fn open(&mut self, x: usize, y: usize) -> bool {
        if self.is_over() || x >= self.w || y >= self.h {
            return false;
        }
        if self.state == State::Ready {
            self.place(x, y);
        }
        let c = self.cell(x, y);
        if c.open || c.flag {
            return false;
        }
        if c.mine {
            self.state = State::Lost(x, y);
            for cell in self.cells.iter_mut().filter(|c| c.mine) {
                cell.open = true;
            }
            return true;
        }
        // 周りに地雷の無いマスからは、つながったマスをまとめて開く
        let mut stack = vec![(x, y)];
        while let Some((cx, cy)) = stack.pop() {
            let i = cy * self.w + cx;
            if self.cells[i].open || self.cells[i].flag {
                continue;
            }
            self.cells[i].open = true;
            self.opened += 1;
            if self.cells[i].around == 0 {
                stack.extend(self.neighbors(cx, cy));
            }
        }
        if self.opened == self.w * self.h - self.mines {
            self.state = State::Won;
            // 残りの地雷には旗を立てて見せる
            for cell in self.cells.iter_mut().filter(|c| c.mine && !c.flag) {
                cell.flag = true;
                self.flags += 1;
            }
        }
        true
    }

    /// 旗を立てる・外す（開いたマスには立てられない）。変わったら true
    pub fn toggle_flag(&mut self, x: usize, y: usize) -> bool {
        if self.is_over() || x >= self.w || y >= self.h {
            return false;
        }
        let cell = &mut self.cells[y * self.w + x];
        if cell.open {
            return false;
        }
        cell.flag = !cell.flag;
        if cell.flag {
            self.flags += 1;
        } else {
            self.flags -= 1;
        }
        true
    }

    /// 地雷の場所（テスト・答え合わせ用）
    pub fn mine_positions(&self) -> Vec<(usize, usize)> {
        (0..self.w * self.h).filter(|&i| self.cells[i].mine).map(|i| (i % self.w, i / self.w)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_open_is_safe_and_places_all_mines() {
        for seed in 0..50 {
            let mut m = Mines::new(9, 9, 10, seed);
            assert_eq!(m.state(), State::Ready);
            assert!(m.open(4, 4));
            assert_eq!(m.state(), State::Playing, "seed {seed}");
            assert_eq!(m.mine_positions().len(), 10);
            // 開いたマスとその周りに地雷は無いので、開いたマスの数字は 0
            assert_eq!(m.cell(4, 4).around, 0);
            for (x, y) in m.mine_positions() {
                assert!(x.abs_diff(4) > 1 || y.abs_diff(4) > 1);
            }
        }
    }

    #[test]
    fn same_seed_same_board() {
        let mut a = Mines::new(9, 9, 10, 7);
        let mut b = Mines::new(9, 9, 10, 7);
        a.open(0, 0);
        b.open(0, 0);
        assert_eq!(a.mine_positions(), b.mine_positions());
    }

    #[test]
    fn numbers_count_neighbors() {
        let mut m = Mines::new(9, 9, 10, 3);
        m.open(4, 4);
        let mines = m.mine_positions();
        for y in 0..9 {
            for x in 0..9 {
                let n = mines.iter().filter(|&&(mx, my)| (mx, my) != (x, y) && mx.abs_diff(x) <= 1 && my.abs_diff(y) <= 1).count();
                assert_eq!(m.cell(x, y).around as usize, n);
            }
        }
    }

    #[test]
    fn flood_fill_opens_region_and_border() {
        let mut m = Mines::new(9, 9, 10, 3);
        m.open(4, 4);
        // 開いた 0 のマスの周りはすべて開いている
        for y in 0..9 {
            for x in 0..9 {
                let c = m.cell(x, y);
                if c.open && c.around == 0 {
                    for (nx, ny) in m.neighbors(x, y) {
                        assert!(m.cell(nx, ny).open);
                    }
                }
                assert!(!(c.open && c.mine));
            }
        }
    }

    #[test]
    fn flags_and_losing() {
        let mut m = Mines::new(9, 9, 10, 11);
        m.open(4, 4);
        let (mx, my) = m.mine_positions()[0];
        assert!(m.toggle_flag(mx, my));
        assert_eq!(m.mines_left(), 9);
        // 旗のマスは開かない
        assert!(!m.open(mx, my));
        assert!(m.toggle_flag(mx, my));
        assert_eq!(m.mines_left(), 10);
        // 開いたマスには旗を立てられない
        assert!(!m.toggle_flag(4, 4));
        assert!(m.open(mx, my));
        assert_eq!(m.state(), State::Lost(mx, my));
        assert!(m.mine_positions().iter().all(|&(x, y)| m.cell(x, y).open));
        // 終わったら何もできない
        assert!(!m.open(0, 0));
        assert!(!m.toggle_flag(0, 0));
    }

    #[test]
    fn winning_by_opening_every_safe_cell() {
        let mut m = Mines::new(9, 9, 10, 5);
        m.open(4, 4);
        let mines = m.mine_positions();
        for y in 0..9 {
            for x in 0..9 {
                if !mines.contains(&(x, y)) {
                    m.open(x, y);
                }
            }
        }
        assert_eq!(m.state(), State::Won);
        assert_eq!(m.mines_left(), 0, "remaining mines are flagged");
    }
}
