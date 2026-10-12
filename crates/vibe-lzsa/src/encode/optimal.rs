//! 最適パース
//!
//! ブロックの先頭から順に、各位置へ「そこまでを書くのに要るビット数が最小の途中の状態」
//! （arrival）を置いていく。位置 i の arrival からは、
//! - リテラルを 1 バイト足して i + 1 へ
//! - i から始まる一致（長さ len）を使って i + len へ
//!
//! 進める。ブロックの終わりの arrival から逆にたどると区切り方が決まる。
//!
//! リテラルの数の欄はリテラルが増えるたびに差分を足す（7 個目で 1 バイト増える、など）ので、
//! arrival は「今続いているリテラルの数」を持つ。トークンの 8 ビットは一致を使うときに足す。
//!
//! LZSA2 の rep-match（前回のオフセットをもう一度使うと、オフセットの欄が 0 ビット）は
//! 「その位置まで来たときの前回のオフセット」で後の大きさが変わる。そこで `rep` のときは
//! 各位置に「前回のオフセットが違う」arrival を `k` 個まで残し、それぞれから rep-match も試す。
//! さらに、いちばん近いオフセットだけでなく遠いオフセットの一致も試し、後で rep-match に
//! なるオフセットを選べるようにする。

use super::emit::{Cmd, lit_ext_bits, match_ext_bits, min_match, offset_bits};
use super::matcher::{Matcher, match_len};
use crate::{MAX_OFFSET, Version};
use alloc::vec;
use alloc::vec::Vec;

/// この長さ以下の一致は、候補より短い長さもすべて試す（短く切ると後がうまく続くことがある）
const ALL_LENGTHS: usize = 64;
/// これより長い一致があったら、その内側の位置からは探さない（長い繰り返しで遅くならないため）
const LONG_MATCH: usize = 256;

const NONE: u32 = u32::MAX;

#[derive(Debug, Clone, Copy)]
struct Arrival {
    cost: u32,
    /// 今続いているリテラルの数
    lits: u32,
    /// 前回の一致のオフセット（0 はまだ無い）
    rep: u32,
    /// どこから来たか（位置と、その位置での arrival の番号）
    from: u32,
    slot: u8,
    /// 来たときの一致の長さ（0 はリテラル）とオフセット
    len: u32,
    dist: u32,
}

const EMPTY: Arrival = Arrival { cost: NONE, lits: 0, rep: 0, from: 0, slot: 0, len: 0, dist: 0 };

impl Arrival {
    fn better_than(&self, o: &Arrival) -> bool {
        self.cost < o.cost || (self.cost == o.cost && self.lits < o.lits)
    }
}

/// 1 つの位置の arrival の並び（コストの小さい順）に `a` を入れる。
/// `rep` のときは前回のオフセットが同じものを 1 つにまとめる
fn insert(slots: &mut [Arrival], a: Arrival, rep: bool) {
    let k = slots.len();
    let mut i = 0;
    while i < k && slots[i].cost != NONE {
        if rep && slots[i].rep == a.rep {
            if a.better_than(&slots[i]) {
                slots[i] = a;
                bubble_up(slots, i);
            }
            return;
        }
        i += 1;
    }
    if i < k {
        slots[i] = a;
        bubble_up(slots, i);
    } else if a.better_than(&slots[k - 1]) {
        slots[k - 1] = a;
        bubble_up(slots, k - 1);
    }
}

fn bubble_up(slots: &mut [Arrival], mut i: usize) {
    while i > 0 && slots[i].better_than(&slots[i - 1]) {
        slots.swap(i, i - 1);
        i -= 1;
    }
}

/// 一致の長さ `lo..=hi` のうち試すもの
fn lengths(lo: usize, hi: usize) -> impl Iterator<Item = usize> {
    (lo..=hi).filter(move |&l| l <= ALL_LENGTHS || l == hi)
}

/// 探し方の設定
#[derive(Clone, Copy)]
pub struct Search {
    /// 各位置に残す arrival の数
    pub arrivals: usize,
    /// LZSA2 の rep-match を考える（前回のオフセットが違う arrival を別々に残す）
    pub rep: bool,
    /// 各位置で試す「長さは伸びないが遠いオフセット」の一致の数
    pub extra: usize,
}

pub fn parse(v: Version, data: &[u8], m: &mut Matcher, bs: usize, be: usize, search: &Search) -> Vec<Cmd> {
    let Search { arrivals: k, rep, extra: n_extra } = *search;
    let n = be - bs;
    let min = min_match(v);
    let mut arr = vec![EMPTY; (n + 1) * k];
    arr[0] = Arrival { cost: 0, ..EMPTY };
    let mut cands = Vec::new();
    let mut extra = Vec::new();
    let mut skip_until = 0;

    for i in 0..n {
        if arr[i * k].cost == NONE || i < skip_until {
            continue;
        }
        let p = bs + i;
        if rep {
            m.find_more(p, be, &mut cands, &mut extra, n_extra, min);
        } else {
            m.find(p, be, &mut cands);
        }
        let max_len = (be - p).min(MAX_OFFSET);
        let mut longest = cands.last().map_or(0, |c| c.len);
        let (here, rest) = arr.split_at_mut((i + 1) * k);
        let here = &here[i * k..];
        for (s, &a) in here.iter().enumerate() {
            if a.cost == NONE {
                break;
            }
            // リテラルを 1 バイト
            let lits = a.lits as usize;
            let cost = a.cost + 8 + lit_ext_bits(v, lits + 1) - lit_ext_bits(v, lits);
            let next = Arrival { cost, lits: a.lits + 1, rep: a.rep, from: i as u32, slot: s as u8, len: 0, dist: 0 };
            insert(&mut rest[..k], next, rep);

            let go = |len: usize, dist: usize, off_bits: u32, rest: &mut [Arrival]| {
                let cost = a.cost + 8 + off_bits + match_ext_bits(v, len);
                let next = Arrival {
                    cost,
                    lits: 0,
                    rep: dist as u32,
                    from: i as u32,
                    slot: s as u8,
                    len: len as u32,
                    dist: dist as u32,
                };
                let at = (len - 1) * k;
                insert(&mut rest[at..at + k], next, rep);
            };
            // 見つけた一致の候補（長さ lo..=c.len はオフセット c.dist で出せる）
            let mut lo = min;
            for c in &cands {
                if c.len >= lo {
                    let off_bits = offset_bits(v, c.dist, rep && a.rep as usize == c.dist);
                    for len in lengths(lo, c.len) {
                        go(len, c.dist, off_bits, rest);
                    }
                    lo = c.len + 1;
                }
            }
            // 長さは伸びないが遠いオフセット（後の rep-match のため。長さはそのまま）
            for c in &extra {
                go(c.len, c.dist, offset_bits(v, c.dist, a.rep as usize == c.dist), rest);
            }
            // 前回のオフセットでの一致（rep-match）
            let r = a.rep as usize;
            if rep && r != 0 && r <= p {
                let len = match_len(data, p - r, p, max_len);
                if len >= min {
                    for len in lengths(min, len) {
                        go(len, r, 0, rest);
                    }
                }
                longest = longest.max(len);
            }
        }
        if longest >= LONG_MATCH {
            skip_until = i + longest;
        }
    }

    // 終わりから逆にたどる
    let mut steps = Vec::new();
    let (mut i, mut s) = (n, 0);
    while i > 0 {
        let a = arr[i * k + s];
        steps.push((a.len as usize, a.dist as usize));
        (i, s) = (a.from as usize, a.slot as usize);
    }
    let mut cmds = Vec::new();
    let mut p = bs;
    let mut lit_start = bs;
    for &(len, dist) in steps.iter().rev() {
        if len == 0 {
            p += 1;
        } else {
            cmds.push(Cmd { lit_start, lits: p - lit_start, len, dist });
            p += len;
            lit_start = p;
        }
    }
    cmds.push(Cmd { lit_start, lits: be - lit_start, len: 0, dist: 0 });
    cmds
}
