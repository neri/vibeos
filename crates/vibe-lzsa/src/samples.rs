//! テスト用のデータ（crate のテストと tools/vibe-lzsa で共有する）
//!
//! 形式の境目（リテラル・一致の長さの欄が伸びる所、LZSA2 のオフセットの形が変わる所、
//! ブロックの境目）をまたぐデータと、種を決めた擬似乱数のデータ。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 種を決めた擬似乱数（xorshift32）
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed.wrapping_mul(2_654_435_761) | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// `0..n`
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u32() as usize) % n
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next_u32() as u8).collect()
    }
}

/// 「リテラルの並び」と「前に出たものの写し」をでたらめに混ぜたデータ
pub fn mixed(seed: u32, size: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    // 使うバイトの種類を減らすと、短い一致が増える
    let alphabet = 1 + r.below(255);
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        if out.is_empty() || r.below(2) == 0 {
            let n = 1 + r.below(40);
            for _ in 0..n {
                out.push((r.below(alphabet)) as u8);
            }
        } else {
            let dist = 1 + r.below(out.len().min(70_000));
            let max = if r.below(8) == 0 { 2000 } else { 40 };
            let len = 2 + r.below(max);
            for _ in 0..len {
                out.push(out[out.len() - dist]);
            }
        }
    }
    out.truncate(size);
    out
}

/// 名前とデータの組
pub fn samples() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = Vec::new();
    let mut r = Rng::new(1);
    v.push(("empty".into(), Vec::new()));
    v.push(("one".into(), b"a".to_vec()));
    v.push(("two-same".into(), b"aa".to_vec()));
    v.push(("three-same".into(), b"aaa".to_vec()));
    v.push(("four-same".into(), b"aaaa".to_vec()));
    v.push(("abab".into(), b"abababababababab".to_vec()));
    v.push(("text".into(), b"the quick brown fox jumps over the lazy dog; the quick brown fox".to_vec()));
    for n in [100, 1000, 65536, 65537, 140_000] {
        v.push((format!("zeros-{n}"), alloc::vec![0; n]));
    }
    for n in [100, 65536, 70_000] {
        v.push((format!("random-{n}"), r.bytes(n)));
    }
    // オフセットの境目: 位置 d から d バイト前の 16 バイトと同じ
    for d in [1, 2, 31, 32, 33, 255, 256, 257, 511, 512, 513, 514, 8703, 8704, 8705, 65535, 65536] {
        let mut b = r.bytes(d);
        let head: Vec<u8> = b[..d.min(16)].to_vec();
        b.extend_from_slice(&head);
        b.extend(r.bytes(8));
        v.push((format!("offset-{d}"), b));
    }
    // 一致の長さの境目
    for len in [2, 3, 8, 9, 17, 18, 23, 24, 25, 254, 255, 256, 257, 511, 512, 513, 1000] {
        let base = r.bytes(1024);
        let mut b = base.clone();
        b.extend_from_slice(&base[..len]);
        b.extend(r.bytes(8));
        v.push((format!("match-{len}"), b));
    }
    // リテラルの長さの境目（同じ 8 バイトの間にでたらめなバイトを挟む）
    for lits in [0, 2, 3, 6, 7, 8, 17, 18, 19, 248, 254, 255, 256, 257, 511, 512, 513, 1000] {
        let mark = r.bytes(8);
        let mut b = Vec::new();
        for _ in 0..3 {
            b.extend_from_slice(&mark);
            b.extend(r.bytes(lits));
        }
        v.push((format!("literals-{lits}"), b));
    }
    for seed in 0..100 {
        let size = if seed % 10 == 0 { 70_000 + seed as usize * 1000 } else { 1 + seed as usize * 97 };
        v.push((format!("mixed-{seed}"), mixed(seed, size)));
    }
    v
}
