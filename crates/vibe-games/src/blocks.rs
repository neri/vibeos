//! 落ち物パズル
//!
//! 盤面は 10 列 × 20 行。ブロックの形は 7 種類（I O T S Z J L）で、7 種類を 1 回ずつ混ぜた袋から
//! 順に出す（同じ種なら同じ順）。形は n × n の箱（I は 4、O は 2、ほかは 3）の中の 4 マスで表し、
//! 回すときは箱の中で時計回りに回す。壁や他のブロックに当たるなら、左右に 1〜2 マスずらして試す。
//!
//! 横 1 列がそろうと消える。点数: 1〜4 列で 100・300・500・800 × レベル、ゆっくり落とすと 1 行 1 点、
//! 一気に落とすと 1 行 2 点。レベルは消した列 10 ごとに上がり、落ちる速さが上がる。
//! 新しいブロックが出る場所がふさがっていたら終わり。

use alloc::vec::Vec;

use crate::Rng;

pub const W: usize = 10;
pub const H: usize = 20;
/// 形の種類の数（色は 1〜7）
pub const KINDS: u8 = 7;

/// 回す前の形（箱の大きさ, 4 マス）
const SHAPES: [(i32, [(i32, i32); 4]); 7] = [
    (4, [(0, 1), (1, 1), (2, 1), (3, 1)]), // I
    (2, [(0, 0), (1, 0), (0, 1), (1, 1)]), // O
    (3, [(1, 0), (0, 1), (1, 1), (2, 1)]), // T
    (3, [(1, 0), (2, 0), (0, 1), (1, 1)]), // S
    (3, [(0, 0), (1, 0), (1, 1), (2, 1)]), // Z
    (3, [(0, 0), (0, 1), (1, 1), (2, 1)]), // J
    (3, [(2, 0), (0, 1), (1, 1), (2, 1)]), // L
];

/// 回したときに試すずらし方
const KICKS: [i32; 5] = [0, -1, 1, -2, 2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Rotate,
    /// 1 行落とす（落とせなければ止める）
    Down,
    /// 一気に落として止める
    Drop,
}

/// 操作・時間の進みの結果
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// ブロックが動いたか
    pub moved: bool,
    /// ブロックが止まって次が出たか
    pub locked: bool,
    /// 消えた列の数
    pub lines: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Piece {
    kind: u8,
    rot: u8,
    x: i32,
    y: i32,
}

impl Piece {
    fn spawn(kind: u8) -> Piece {
        let (n, cells) = SHAPES[kind as usize];
        // 一番上のマスが盤面の 1 行目に来るようにする
        let top = cells.iter().map(|c| c.1).min().unwrap_or(0);
        Piece { kind, rot: 0, x: (W as i32 - n) / 2, y: -top }
    }

    fn cells(&self) -> [(i32, i32); 4] {
        let mut out = shape_cells(self.kind, self.rot);
        for c in &mut out {
            c.0 += self.x;
            c.1 += self.y;
        }
        out
    }
}

/// 形 `kind` を `rot` 回時計回りに回したときの、箱の中の 4 マス
pub fn shape_cells(kind: u8, rot: u8) -> [(i32, i32); 4] {
    let (n, mut cells) = SHAPES[kind as usize % SHAPES.len()];
    for _ in 0..rot % 4 {
        for c in &mut cells {
            *c = (n - 1 - c.1, c.0);
        }
    }
    cells
}

pub struct Blocks {
    board: [[u8; W]; H],
    piece: Piece,
    queue: Vec<u8>,
    rng: Rng,
    score: u32,
    lines: u32,
    over: bool,
}

impl Blocks {
    pub fn new(seed: u32) -> Blocks {
        Self::with_queue(seed, &[])
    }

    /// 最初に出す形を決めて始める（テスト用。その後は袋から）
    pub fn with_queue(seed: u32, first: &[u8]) -> Blocks {
        let mut b = Blocks {
            board: [[0; W]; H],
            piece: Piece::spawn(0),
            queue: Vec::new(),
            rng: Rng::new(seed),
            score: 0,
            lines: 0,
            over: false,
        };
        b.queue.extend(first.iter().rev().map(|k| k % KINDS));
        let kind = b.take_kind();
        b.piece = Piece::spawn(kind);
        b
    }

    /// 次の形を取り出す（足りなければ 7 種類を混ぜた袋を足す）。`queue` は後ろから使う
    fn take_kind(&mut self) -> u8 {
        if self.queue.len() < 2 {
            let mut bag: Vec<u8> = (0..KINDS).collect();
            for i in (1..bag.len()).rev() {
                let j = self.rng.below(i as u32 + 1) as usize;
                bag.swap(i, j);
            }
            // 後ろから使うので、袋の順を前に足す
            bag.reverse();
            bag.append(&mut self.queue);
            self.queue = bag;
        }
        self.queue.pop().unwrap()
    }

    /// 盤面のマス（0 は空、1〜7 は止まったブロックの色）
    pub fn cell(&self, x: usize, y: usize) -> u8 {
        self.board[y][x]
    }

    /// 落ちているブロックの 4 マス（盤面の座標。上にはみ出すと y が負）と色
    pub fn piece(&self) -> ([(i32, i32); 4], u8) {
        (self.piece.cells(), self.piece.kind + 1)
    }

    /// 次に出る形
    pub fn next_kind(&self) -> u8 {
        *self.queue.last().unwrap()
    }

    /// 一気に落としたときに止まる位置の 4 マス（影の表示用）
    pub fn ghost(&self) -> [(i32, i32); 4] {
        let mut p = self.piece;
        while self.fits(Piece { y: p.y + 1, ..p }) {
            p.y += 1;
        }
        p.cells()
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn lines(&self) -> u32 {
        self.lines
    }

    pub fn level(&self) -> u32 {
        self.lines / 10 + 1
    }

    pub fn is_over(&self) -> bool {
        self.over
    }

    /// 1 行落ちるまでの時間
    pub fn fall_interval_ms(&self) -> u32 {
        800u32.saturating_sub((self.level() - 1) * 70).max(100)
    }

    fn fits(&self, p: Piece) -> bool {
        p.cells().iter().all(|&(x, y)| {
            (0..W as i32).contains(&x) && y < H as i32 && (y < 0 || self.board[y as usize][x as usize] == 0)
        })
    }

    pub fn apply(&mut self, m: Move) -> Outcome {
        if self.over {
            return Outcome::default();
        }
        let p = self.piece;
        match m {
            Move::Left | Move::Right => {
                let dx = if m == Move::Left { -1 } else { 1 };
                self.try_move(Piece { x: p.x + dx, ..p })
            }
            Move::Rotate => {
                let rot = (p.rot + 1) % 4;
                for dx in KICKS {
                    let o = self.try_move(Piece { rot, x: p.x + dx, ..p });
                    if o.moved {
                        return o;
                    }
                }
                Outcome::default()
            }
            Move::Down => {
                let o = self.try_move(Piece { y: p.y + 1, ..p });
                if o.moved {
                    self.score += 1;
                    o
                } else {
                    self.lock()
                }
            }
            Move::Drop => {
                let mut rows = 0;
                while self.fits(Piece { y: self.piece.y + 1, ..self.piece }) {
                    self.piece.y += 1;
                    rows += 1;
                }
                self.score += 2 * rows;
                let o = self.lock();
                Outcome { moved: rows > 0, ..o }
            }
        }
    }

    /// 時間が 1 段進んだ（1 行落とす。落とせなければ止める）
    pub fn tick(&mut self) -> Outcome {
        if self.over {
            return Outcome::default();
        }
        let p = self.piece;
        let o = self.try_move(Piece { y: p.y + 1, ..p });
        if o.moved { o } else { self.lock() }
    }

    fn try_move(&mut self, p: Piece) -> Outcome {
        if self.fits(p) {
            self.piece = p;
            Outcome { moved: true, ..Outcome::default() }
        } else {
            Outcome::default()
        }
    }

    /// 今のブロックを盤面に止め、そろった列を消して次のブロックを出す
    fn lock(&mut self) -> Outcome {
        let color = self.piece.kind + 1;
        for (x, y) in self.piece.cells() {
            if y < 0 {
                self.over = true; // 盤面の上にはみ出して止まった
                continue;
            }
            self.board[y as usize][x as usize] = color;
        }
        let mut cleared = 0;
        let mut y = H;
        while y > 0 {
            y -= 1;
            if self.board[y].iter().all(|&c| c != 0) {
                // この列を消して上を 1 行ずつ下げる（同じ y をもう一度見る）
                for yy in (1..=y).rev() {
                    self.board[yy] = self.board[yy - 1];
                }
                self.board[0] = [0; W];
                cleared += 1;
                y += 1;
            }
        }
        if cleared > 0 {
            const POINTS: [u32; 5] = [0, 100, 300, 500, 800];
            self.score += POINTS[cleared.min(4)] * self.level();
            self.lines += cleared as u32;
        }
        let kind = self.take_kind();
        self.piece = Piece::spawn(kind);
        if !self.fits(self.piece) {
            self.over = true;
        }
        Outcome { moved: false, locked: true, lines: cleared as u32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const I: u8 = 0;
    const O: u8 = 1;
    const T: u8 = 2;

    #[test]
    fn rotations_stay_in_the_box() {
        for kind in 0..KINDS {
            let n = SHAPES[kind as usize].0;
            for rot in 0..4 {
                for (x, y) in shape_cells(kind, rot) {
                    assert!((0..n).contains(&x) && (0..n).contains(&y));
                }
            }
            // 4 回で元に戻る
            assert_eq!(shape_cells(kind, 4), shape_cells(kind, 0));
        }
        // T を 1 回: 縦の棒の右に出っ張り
        let mut t = shape_cells(T, 1).to_vec();
        t.sort();
        assert_eq!(t, [(1, 0), (1, 1), (1, 2), (2, 1)]);
    }

    #[test]
    fn bag_gives_every_kind_once_per_seven() {
        let mut b = Blocks::new(42);
        let mut seen = [0; 7];
        seen[b.piece().1 as usize - 1] += 1;
        for _ in 0..13 {
            seen[b.take_kind() as usize] += 1;
        }
        assert_eq!(seen, [2; 7]);
        // 同じ種なら同じ並び
        let kinds = |seed| {
            let mut b = Blocks::new(seed);
            (0..7).map(|_| { let k = b.piece().1; b.apply(Move::Drop); k }).collect::<Vec<_>>()
        };
        assert_eq!(kinds(5), kinds(5));
        assert_ne!(kinds(5), kinds(6));
    }

    #[test]
    fn spawn_and_walls() {
        let mut b = Blocks::with_queue(1, &[O]);
        let (cells, color) = b.piece();
        assert_eq!(color, O + 1);
        let mut c = cells.to_vec();
        c.sort();
        assert_eq!(c, [(4, 0), (4, 1), (5, 0), (5, 1)]);
        for _ in 0..10 {
            b.apply(Move::Left);
        }
        assert_eq!(b.piece().0.iter().map(|c| c.0).min(), Some(0));
        assert!(!b.apply(Move::Left).moved);
        // I は 1 行目に横向きで出る
        let b = Blocks::with_queue(1, &[I]);
        assert!(b.piece().0.iter().all(|&(_, y)| y == 0));
    }

    #[test]
    fn drop_lands_on_the_bottom_and_scores() {
        let mut b = Blocks::with_queue(1, &[O, O]);
        assert_eq!(b.ghost().iter().map(|c| c.1).max(), Some(19));
        let o = b.apply(Move::Drop);
        assert!(o.locked && o.moved);
        assert_eq!(b.cell(4, 19), O + 1);
        assert_eq!(b.cell(5, 18), O + 1);
        assert_eq!(b.score(), 2 * 18);
        // 2 つ目は 1 つ目の上に乗る
        b.apply(Move::Down);
        assert_eq!(b.score(), 36 + 1);
        b.apply(Move::Drop);
        assert_eq!(b.cell(4, 17), O + 1);
    }

    #[test]
    fn full_lines_are_cleared() {
        // O を 5 つ横に並べると下の 2 列がそろって消える
        let mut b = Blocks::with_queue(1, &[O, O, O, O, O]);
        for target in [0, 2, 4, 6, 8] {
            let x = b.piece().0.iter().map(|c| c.0).min().unwrap();
            let mv = if target < x { Move::Left } else { Move::Right };
            for _ in 0..(target - x).abs() {
                b.apply(mv);
            }
            let o = b.apply(Move::Drop);
            if target == 8 {
                assert_eq!(o.lines, 2);
            }
        }
        assert_eq!(b.lines(), 2);
        assert!((0..W).all(|x| b.cell(x, 19) == 0 && b.cell(x, 18) == 0));
        // 2 列で 300 点（レベル 1）と、落とした分（5 回 × 18 行 × 2）
        assert_eq!(b.score(), 300 + 5 * 36);
    }

    #[test]
    fn rotation_kicks_off_the_wall() {
        let mut b = Blocks::with_queue(1, &[I]);
        b.apply(Move::Rotate); // 縦
        for _ in 0..10 {
            b.apply(Move::Right);
        }
        assert_eq!(b.piece().0.iter().map(|c| c.0).max(), Some(9));
        // 右の壁際で横に戻すと、左にずれて収まる
        assert!(b.apply(Move::Rotate).moved);
        assert!(b.piece().0.iter().all(|&(x, _)| (0..10).contains(&x)));
    }

    #[test]
    fn game_over_when_the_stack_reaches_the_top() {
        let mut b = Blocks::new(9);
        let mut drops = 0;
        while !b.is_over() {
            b.apply(Move::Drop);
            drops += 1;
            assert!(drops < 200, "never ended");
        }
        // 終わったら何も動かない
        assert_eq!(b.apply(Move::Left), Outcome::default());
        assert_eq!(b.tick(), Outcome::default());
    }

    #[test]
    fn gravity_and_speed() {
        let mut b = Blocks::with_queue(1, &[T]);
        let y0 = b.piece().0[0].1;
        assert!(b.tick().moved);
        assert_eq!(b.piece().0[0].1, y0 + 1);
        assert_eq!(b.fall_interval_ms(), 800);
        b.lines = 25;
        assert_eq!(b.level(), 3);
        assert_eq!(b.fall_interval_ms(), 660);
        b.lines = 1000;
        assert_eq!(b.fall_interval_ms(), 100);
    }
}
