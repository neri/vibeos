//! gzip（RFC 1952）の展開。アプリ（`/BIN/*.ELF`）は gzip で圧縮して置く（docs/plans/app-compress.md）
//!
//! ```text
//! 1F 8B 08 FLG MTIME(4) XFL OS [FEXTRA][FNAME][FCOMMENT][FHCRC]  deflate のデータ  CRC32(4) ISIZE(4)
//! ```
//!
//! 枠（ヘッダ・CRC32・大きさ）はここで読み、中の deflate（RFC 1951）は `miniz_oxide` で展開する。
//! 展開後の大きさは末尾の ISIZE で分かるので、先にその大きさの出力先を取り、越えて展開しようと
//! したら壊れているとする。メンバーは 1 つだけ（`gzip` コマンドが 1 つのファイルから作る形）。

use alloc::boxed::Box;
use alloc::vec::Vec;
use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

use crate::crc32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GzipError {
    /// 先頭が 1F 8B でない
    NotGzip,
    /// ヘッダが壊れている・deflate 以外の方式・途中で切れている
    BadHeader,
    /// 展開後の大きさ（ISIZE）が上限を越える
    TooLarge,
    /// メモリが足りない
    OutOfMemory,
    /// deflate のデータが壊れている
    BadData,
    /// 展開した大きさが ISIZE と違う・deflate の後ろに余分なデータがある
    SizeMismatch,
    /// 展開した中身の CRC32 が違う
    BadCrc,
}

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const TRAILER: usize = 8;

/// 先頭が gzip の印（1F 8B）か
pub fn is_gzip(b: &[u8]) -> bool {
    b.starts_with(&[0x1F, 0x8B])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// ヘッダを読み飛ばし、deflate のデータの始まりを返す
fn skip_header(b: &[u8]) -> Result<usize, GzipError> {
    if !is_gzip(b) {
        return Err(GzipError::NotGzip);
    }
    if b.len() < 10 || b[2] != 8 || b[3] & 0xE0 != 0 {
        return Err(GzipError::BadHeader);
    }
    let flg = b[3];
    let mut p = 10;
    if flg & FEXTRA != 0 {
        let xlen = *b.get(p).ok_or(GzipError::BadHeader)? as usize
            | (*b.get(p + 1).ok_or(GzipError::BadHeader)? as usize) << 8;
        p += 2 + xlen;
    }
    for flag in [FNAME, FCOMMENT] {
        if flg & flag != 0 {
            let rest = b.get(p..).ok_or(GzipError::BadHeader)?;
            p += rest.iter().position(|&c| c == 0).ok_or(GzipError::BadHeader)? + 1;
        }
    }
    if flg & FHCRC != 0 {
        let crc = b.get(p..p + 2).ok_or(GzipError::BadHeader)?;
        // ヘッダの CRC16 = ヘッダの CRC32 の下位 16 ビット
        if u16::from_le_bytes([crc[0], crc[1]]) != crc32(&b[..p]) as u16 {
            return Err(GzipError::BadHeader);
        }
        p += 2;
    }
    if p + TRAILER > b.len() {
        return Err(GzipError::BadHeader);
    }
    Ok(p)
}

/// deflate で 1 バイトから出せる最大のバイト数（長さ 258 の一致を 2 ビットで書いたとき）
const MAX_EXPANSION: usize = 1032;

/// 展開する。展開後の大きさが `max_size` を越えるもの、deflate の部分の大きさからは出せないもの
/// （途中で切れたファイルの末尾をサイズと読んだときなど）は、メモリを取りに行かずに `TooLarge`
pub fn gunzip(src: &[u8], max_size: usize) -> Result<Vec<u8>, GzipError> {
    let start = skip_header(src)?;
    let end = src.len() - TRAILER;
    let crc = u32_at(src, end);
    let size = u32_at(src, end + 4) as usize;
    if size > max_size || size > (end - start).saturating_mul(MAX_EXPANSION) {
        return Err(GzipError::TooLarge);
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|_| GzipError::OutOfMemory)?;
    out.resize(size, 0);
    // 展開の状態は 11KB ほどあるので、カーネルのスタックに置かずヒープに取る
    let mut state = Box::<DecompressorOxide>::default();
    let flags = inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, read, written) = decompress(&mut state, &src[start..end], &mut out, 0, flags);
    match status {
        TINFLStatus::Done => {}
        // 出力先（ISIZE の大きさ）を越えて展開しようとした
        TINFLStatus::HasMoreOutput => return Err(GzipError::SizeMismatch),
        _ => return Err(GzipError::BadData),
    }
    if written != size || start + read != end {
        return Err(GzipError::SizeMismatch);
    }
    if crc32(&out) != crc {
        return Err(GzipError::BadCrc);
    }
    Ok(out)
}
