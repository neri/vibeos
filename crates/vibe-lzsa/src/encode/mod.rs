//! 圧縮器
//!
//! 入力を 64KB ごとのブロックに分け、ブロックごとに「どこをリテラルにし、どこを一致にするか」
//! （区切り方）を決めて書く。区切り方の決め方は [`Parser`] で選ぶ。

mod emit;
mod matcher;
mod optimal;

use crate::format::*;
use crate::{BLOCK_SIZE, Version};
use alloc::vec::Vec;
use emit::{Cmd, min_match, write_block};
use matcher::{Match, Matcher};

/// 区切り方の決め方
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parser {
    /// 見つけたいちばん長い一致をすぐ使う
    Greedy,
    /// 1 つ先の位置の一致の方が長ければ、今の位置はリテラルにする（zlib と同じ）
    Lazy,
    /// ブロック全体で書くビット数が最小になる区切り方を DP で探す（LZSA2 の rep-match は考えない）
    Optimal,
    /// LZSA2 の rep-match も考えた最適パース（LZSA1 では `Optimal` と同じ）
    OptimalRep,
}

/// 圧縮の設定
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub version: Version,
    /// raw block（64KB まで。フレームの枠を付けず、終わりに EOD の印）
    pub raw: bool,
    pub parser: Parser,
    /// 一致の候補を探すとき、たどる位置の数の上限
    pub depth: usize,
    /// `OptimalRep` で、各位置に残す途中の状態（前回のオフセットが違うもの）の数
    pub arrivals: usize,
    /// `OptimalRep` で、各位置で試す「長さは伸びないが遠いオフセット」の一致の数
    pub extra: usize,
}

impl Options {
    pub fn new(version: Version) -> Self {
        Options { version, raw: false, parser: Parser::OptimalRep, depth: 4096, arrivals: 32, extra: 16 }
    }
}

/// 圧縮できなかった
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressError {
    /// raw block で、入力か圧縮後が [`BLOCK_SIZE`] を超える（本家の `lzsa -r` と同じ制限。
    /// 縮まないデータは raw block では無圧縮にできない）
    TooLarge,
}

/// 圧縮する
pub fn compress(data: &[u8], opt: &Options) -> Result<Vec<u8>, CompressError> {
    let mut m = Matcher::new(data, opt.depth);
    let mut out = Vec::new();
    if opt.raw {
        if data.len() > BLOCK_SIZE {
            return Err(CompressError::TooLarge);
        }
        let cmds = parse(data, &mut m, 0, data.len(), opt);
        write_block(opt.version, data, &cmds, true, &mut out);
        return if out.len() > BLOCK_SIZE { Err(CompressError::TooLarge) } else { Ok(out) };
    }
    out.extend_from_slice(&MAGIC);
    out.push(match opt.version {
        Version::V1 => 0x00,
        Version::V2 => 0x20,
    });
    let mut block = Vec::new();
    for bs in (0..data.len()).step_by(BLOCK_SIZE) {
        let be = (bs + BLOCK_SIZE).min(data.len());
        let cmds = parse(data, &mut m, bs, be, opt);
        block.clear();
        write_block(opt.version, data, &cmds, false, &mut block);
        // 縮まなかったブロックは無圧縮のフレームにする
        let (body, flag) =
            if block.len() >= be - bs { (&data[bs..be], FRAME_UNCOMPRESSED) } else { (&block[..], 0) };
        out.push(body.len() as u8);
        out.push((body.len() >> 8) as u8);
        out.push(flag | (body.len() >> 16) as u8);
        out.extend_from_slice(body);
    }
    out.extend_from_slice(&[0, 0, 0]);
    Ok(out)
}

/// ブロック `data[bs..be]` の区切り方を決める
fn parse(data: &[u8], m: &mut Matcher, bs: usize, be: usize, opt: &Options) -> Vec<Cmd> {
    match opt.parser {
        Parser::Greedy => parse_greedy(opt.version, m, bs, be, false),
        Parser::Lazy => parse_greedy(opt.version, m, bs, be, true),
        Parser::Optimal => {
            let search = optimal::Search { arrivals: 1, rep: false, extra: 0 };
            optimal::parse(opt.version, data, m, bs, be, &search)
        }
        Parser::OptimalRep => {
            // rep-match の無い LZSA1 では、前回のオフセットで arrival を分ける意味が無い
            let search = match opt.version {
                Version::V1 => optimal::Search { arrivals: 1, rep: false, extra: 0 },
                Version::V2 => optimal::Search { arrivals: opt.arrivals, rep: true, extra: opt.extra },
            };
            optimal::parse(opt.version, data, m, bs, be, &search)
        }
    }
}

fn longest(cands: &[Match], min: usize) -> Option<Match> {
    cands.last().copied().filter(|c| c.len >= min)
}

fn parse_greedy(
    v: Version,
    m: &mut Matcher,
    bs: usize,
    be: usize,
    lazy: bool,
) -> Vec<Cmd> {
    let min = min_match(v);
    let mut cmds = Vec::new();
    let mut cands = Vec::new();
    let mut lit_start = bs;
    let mut p = bs;
    while p < be {
        m.find(p, be, &mut cands);
        let Some(here) = longest(&cands, min) else {
            p += 1;
            continue;
        };
        if lazy && p + 1 < be {
            m.find(p + 1, be, &mut cands);
            if longest(&cands, min).is_some_and(|next| next.len > here.len) {
                p += 1;
                continue;
            }
        }
        cmds.push(Cmd { lit_start, lits: p - lit_start, len: here.len, dist: here.dist });
        p += here.len;
        lit_start = p;
    }
    cmds.push(Cmd { lit_start, lits: be - lit_start, len: 0, dist: 0 });
    cmds
}
