//! 形式の定数（展開器と圧縮器で共有する）

/// ストリームのヘッダ（signature 2 バイト + traits）
pub const MAGIC: [u8; 2] = [0x7b, 0x9e];
/// フレームの長さの欄の「無圧縮」ビット（3 バイト目の bit 7）
pub const FRAME_UNCOMPRESSED: u8 = 0x80;

pub const MIN_MATCH_V1: usize = 3;
pub const MIN_MATCH_V2: usize = 2;

/// LZSA1 のトークン: O|LLL|MMMM
pub const V1_LONG_OFFSET: u8 = 0x80;
pub const V1_LIT_RUN: usize = 7;
pub const V1_MATCH_RUN: usize = 15;

/// LZSA2 のトークン: XYZ|LL|MMM
pub const V2_LIT_RUN: usize = 3;
pub const V2_MATCH_RUN: usize = 7;
/// XYZ = 111（前回のオフセットをもう一度使う）
pub const V2_REP: u8 = 0xe0;
