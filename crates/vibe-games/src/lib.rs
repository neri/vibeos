//! ゲームの中身（盤面と規則）。描画・入力・時間はアプリが受け持つので、ホストでテストできる
//!
//! - `mines`: マインスイーパ
//! - `blocks`: 落ち物パズル（10x20 の盤面に 7 種類の 4 つのブロックの形）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod blocks;
pub mod mines;

/// 簡単な乱数（xorshift32）。同じ種なら同じ並び
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        // 0 だと 0 のままなので避ける。種の近い値でも並びが離れるよう少し混ぜる
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9) | 1);
        r.next();
        r
    }

    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// 0..n
    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }
}
