//! LZSA1 / LZSA2 の展開と圧縮
//!
//! 形式は本家（https://github.com/emmanuel-marty/lzsa）の仕様書
//! `BlockFormat_LZSA1.md`, `BlockFormat_LZSA2.md`, `StreamFormat.md` に従う。
//! 本家の `lzsa` コマンドと互換で、どちらで圧縮したものも互いに展開できる。
//!
//! - ストリーム形式: ヘッダ（3 バイト）+ フレーム（展開後 64KB まで）の並び + 終わりの印。
//!   フレームは前のフレームの中身を参照できる（オフセットは最大 65535）
//! - raw block: フレームの枠の無いブロック 1 つ（展開後 64KB まで）。終わりは EOD の印
//!
//! 展開器（[`decompress`], [`decompress_raw`]）は `no_std` でヒープを使わない。
//! 圧縮器（[`compress`]）は `alloc` の feature の中にある。

#![cfg_attr(not(test), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod decode;
#[cfg(feature = "alloc")]
mod encode;
mod format;
#[cfg(feature = "alloc")]
#[doc(hidden)]
pub mod samples;

pub use decode::{decompress, decompress_block, decompress_raw};
#[cfg(feature = "alloc")]
pub use decode::{decompress_raw_to_vec, decompress_to_vec};
#[cfg(feature = "alloc")]
pub use encode::{CompressError, Options, Parser, compress};

/// ブロックの形式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    /// LZSA1: バイト単位。展開が速い
    V1,
    /// LZSA2: ニブル単位の長さ・オフセットと rep-match。よく縮む
    V2,
}

/// 展開の失敗
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 入力が途中で切れている
    Truncated,
    /// ストリームのヘッダが違う
    BadHeader,
    /// フレームの長さの欄が壊れている
    BadFrame,
    /// オフセットが出力の先頭より前を指している
    BadOffset,
    /// 出力先が足りない
    OutputFull,
}

/// 1 つのブロック（フレーム）が展開後に取れる最大の大きさ
pub const BLOCK_SIZE: usize = 65536;
/// オフセットの最大
pub const MAX_OFFSET: usize = 65535;
