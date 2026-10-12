//! 一致の候補を探す（ハッシュチェーン）
//!
//! 先頭 2 バイトが同じ位置を「近い順」につないでおき、近い方からたどって比べる。
//! 長さが前の候補より伸びたときだけ候補に加えるので、候補は「長さが増える順・その長さを
//! 出せるいちばん近いオフセット」の並びになる。オフセットは近いほど短く書ける（LZSA1 は
//! 256 まで 1 バイト、LZSA2 は 32・512・8704 で段がある）ので、長さごとに最も近いものが要る。

use crate::MAX_OFFSET;
use alloc::vec;
use alloc::vec::Vec;

/// 一致の候補
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub len: usize,
    pub dist: usize,
}

pub struct Matcher<'a> {
    data: &'a [u8],
    /// 先頭 2 バイトごとの、いちばん最近に登録した位置 + 1（0 は無し）
    head: Vec<u32>,
    /// 同じ先頭 2 バイトの、1 つ前の位置 + 1
    prev: Vec<u32>,
    /// ここより前の位置は登録済み
    inserted: usize,
    /// たどる候補の数の上限
    depth: usize,
}

impl<'a> Matcher<'a> {
    pub fn new(data: &'a [u8], depth: usize) -> Self {
        Matcher { data, head: vec![0; 1 << 16], prev: vec![0; data.len()], inserted: 0, depth }
    }

    fn key(&self, p: usize) -> usize {
        self.data[p] as usize | (self.data[p + 1] as usize) << 8
    }

    /// `p` より前の位置をすべて登録する
    pub fn insert_until(&mut self, p: usize) {
        while self.inserted < p {
            let q = self.inserted;
            if q + 1 < self.data.len() {
                let k = self.key(q);
                self.prev[q] = self.head[k];
                self.head[k] = q as u32 + 1;
            }
            self.inserted += 1;
        }
    }

    /// `p` から始まる一致の候補を `out` に入れる（長さが増える順）。一致は `limit` で終わる
    /// （ブロックの終わり）。`p` より前を登録してから呼ぶこと
    pub fn find(&mut self, p: usize, limit: usize, out: &mut Vec<Match>) {
        self.find_more(p, limit, out, &mut Vec::new(), 0, usize::MAX);
    }

    /// `find` に加えて、長さが伸びなかった遠い候補（長さ `extra_min` 以上）を近い順に `extra` へ
    /// `max_extra` 個まで入れる。LZSA2 では、近いオフセットの代わりに遠いオフセットを使うと
    /// 後の一致が rep-match（オフセットの欄が 0 ビット）になることがある
    pub fn find_more(
        &mut self,
        p: usize,
        limit: usize,
        out: &mut Vec<Match>,
        extra: &mut Vec<Match>,
        max_extra: usize,
        extra_min: usize,
    ) {
        out.clear();
        extra.clear();
        self.insert_until(p);
        let max_len = (limit - p).min(MAX_OFFSET);
        if max_len < 2 {
            return;
        }
        let data = self.data;
        let mut best = 1;
        let mut q1 = self.head[self.key(p)];
        let mut steps = 0;
        while q1 != 0 && steps < self.depth {
            let q = q1 as usize - 1;
            let dist = p - q;
            if dist > MAX_OFFSET {
                break;
            }
            steps += 1;
            if data[q + best] == data[p + best] {
                let len = match_len(data, q, p, max_len);
                if len > best {
                    best = len;
                    out.push(Match { len, dist });
                    if len == max_len {
                        break;
                    }
                    q1 = self.prev[q];
                    continue;
                }
            }
            if extra.len() < max_extra {
                let len = match_len(data, q, p, max_len);
                if len >= extra_min {
                    extra.push(Match { len, dist });
                }
            }
            q1 = self.prev[q];
        }
    }
}

/// `q` と `p` から何バイト同じか（`max_len` まで。`q < p` で、重なってよい）
pub fn match_len(data: &[u8], q: usize, p: usize, max_len: usize) -> usize {
    let a = &data[q..q + max_len];
    let b = &data[p..p + max_len];
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}
