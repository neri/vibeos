//! PNG の読み方（ブラウザの画像。BMP は `vibe_gfx::bmp`）
//!
//! - 塊（チャンク）: IHDR・PLTE・tRNS・IDAT・IEND を読み、ほかは飛ばす。CRC32 を確かめる
//! - 色の種類: グレー（1・2・4・8・16 ビット）・RGB（8・16）・パレット（1・2・4・8）・グレー + アルファ
//!   （8・16）・RGBA（8・16）。tRNS（パレットのアルファ・透明にする 1 色）も見る
//! - 並べ方: ふつうと Adam7（飛び飛びに 7 回）
//! - IDAT をつなげたものを zlib として展開し（`miniz_oxide`。出力の大きさは IHDR から決まるので先に取る）、
//!   行ごとのフィルタ（None・Sub・Up・Average・Paeth）を戻す
//! - 結果は 0x00RRGGBB。アルファは地の色 `bg` の上に重ねて消す（ページの地に描くだけなので）
//!
//! 壊れたデータは `PngError` で、panic しない。大きさは `max_pixels` まで。

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngError {
    NotPng,
    BadChunk,
    BadCrc,
    /// IHDR が無い・おかしい・扱えない組み合わせ
    BadHeader,
    TooLarge,
    /// 展開できない・大きさが合わない
    BadData,
    BadFilter,
    /// パレットが無い・番号がパレットの外
    BadPalette,
}

impl PngError {
    pub fn message(self) -> &'static str {
        match self {
            PngError::NotPng => "not a PNG file",
            PngError::BadChunk => "broken PNG chunk",
            PngError::BadCrc => "PNG CRC mismatch",
            PngError::BadHeader => "unsupported PNG header",
            PngError::TooLarge => "image too large",
            PngError::BadData => "broken PNG data",
            PngError::BadFilter => "broken PNG filter",
            PngError::BadPalette => "broken PNG palette",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// 0x00RRGGBB
    pub pixels: Vec<u32>,
}

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub fn is_png(data: &[u8]) -> bool {
    data.starts_with(SIGNATURE)
}

struct Header {
    width: usize,
    height: usize,
    depth: u8,
    color: u8,
    interlace: bool,
}

impl Header {
    /// 1 画素の成分の数
    fn channels(&self) -> usize {
        match self.color {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    fn bits_per_pixel(&self) -> usize {
        self.channels() * self.depth as usize
    }

    /// 幅 `w` の 1 行のバイト数（フィルタの種類の 1 バイトを除く）
    fn stride(&self, w: usize) -> usize {
        (w * self.bits_per_pixel()).div_ceil(8)
    }
}

/// Adam7 の 7 回の (x の始め, y の始め, x の間, y の間)
const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// 各回の (幅, 高さ)。ふつうの並べ方なら 1 回
fn passes(h: &Header) -> Vec<(usize, usize, usize, usize, usize, usize)> {
    if !h.interlace {
        return vec![(0, 0, 1, 1, h.width, h.height)];
    }
    ADAM7
        .iter()
        .map(|&(x0, y0, dx, dy)| {
            let w = (h.width + dx - 1 - x0) / dx;
            let hh = (h.height + dy - 1 - y0) / dy;
            (x0, y0, dx, dy, if h.width > x0 { w } else { 0 }, if h.height > y0 { hh } else { 0 })
        })
        .collect()
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// PNG を読む。`bg` はアルファを重ねる地の色
pub fn decode(data: &[u8], max_pixels: usize, bg: u32) -> Result<Image, PngError> {
    if !is_png(data) {
        return Err(PngError::NotPng);
    }
    let mut pos = SIGNATURE.len();
    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();
    let mut ended = false;
    while pos + 12 <= data.len() {
        let len = be32(&data[pos..]) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body_end = pos.checked_add(8 + len).filter(|&e| e + 4 <= data.len()).ok_or(PngError::BadChunk)?;
        let body = &data[pos + 8..body_end];
        if vibe_pack::crc32(&data[pos + 4..body_end]) != be32(&data[body_end..]) {
            return Err(PngError::BadCrc);
        }
        pos = body_end + 4;
        match kind {
            b"IHDR" => {
                if body.len() != 13 || header.is_some() {
                    return Err(PngError::BadHeader);
                }
                let h = Header {
                    width: be32(body) as usize,
                    height: be32(&body[4..]) as usize,
                    depth: body[8],
                    color: body[9],
                    interlace: body[12] == 1,
                };
                let ok_depth = match h.color {
                    0 => matches!(h.depth, 1 | 2 | 4 | 8 | 16),
                    3 => matches!(h.depth, 1 | 2 | 4 | 8),
                    2 | 4 | 6 => matches!(h.depth, 8 | 16),
                    _ => false,
                };
                if !ok_depth || body[10] != 0 || body[11] != 0 || body[12] > 1 || h.width == 0 || h.height == 0 {
                    return Err(PngError::BadHeader);
                }
                if h.width.checked_mul(h.height).is_none_or(|n| n > max_pixels) {
                    return Err(PngError::TooLarge);
                }
                header = Some(h);
            }
            b"PLTE" => {
                if body.len() % 3 != 0 || body.len() > 256 * 3 {
                    return Err(PngError::BadPalette);
                }
                palette = body.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
            }
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => {
                ended = true;
                break;
            }
            _ => {}
        }
    }
    let h = header.ok_or(PngError::BadHeader)?;
    if !ended || idat.is_empty() {
        return Err(PngError::BadData);
    }
    if h.color == 3 && palette.is_empty() {
        return Err(PngError::BadPalette);
    }
    let passes = passes(&h);
    let raw_size: usize = passes.iter().filter(|p| p.4 > 0 && p.5 > 0).map(|p| p.5 * (1 + h.stride(p.4))).sum();
    let mut raw = Vec::new();
    raw.try_reserve_exact(raw_size).map_err(|_| PngError::TooLarge)?;
    raw.resize(raw_size, 0);
    // 展開の状態は 11KB ほどあるのでヒープに取る
    let mut state = Box::<DecompressorOxide>::default();
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, _, written) = decompress(&mut state, &idat, &mut raw, 0, flags);
    if !matches!(status, TINFLStatus::Done) || written != raw_size {
        return Err(PngError::BadData);
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(h.width * h.height).map_err(|_| PngError::TooLarge)?;
    pixels.resize(h.width * h.height, bg);
    let bpp = h.bits_per_pixel().div_ceil(8);
    let mut at = 0;
    for &(x0, y0, dx, dy, pw, ph) in &passes {
        if pw == 0 || ph == 0 {
            continue;
        }
        let stride = h.stride(pw);
        let rows = &mut raw[at..at + ph * (1 + stride)];
        at += ph * (1 + stride);
        unfilter(rows, stride, bpp)?;
        for py in 0..ph {
            let row = &rows[py * (1 + stride) + 1..(py + 1) * (1 + stride)];
            for px in 0..pw {
                let c = pixel(&h, row, px, &palette, &trns, bg)?;
                pixels[(y0 + py * dy) * h.width + x0 + px * dx] = c;
            }
        }
    }
    Ok(Image { width: h.width, height: h.height, pixels })
}

/// 行ごとのフィルタを戻す（`rows` は各行の頭にフィルタの種類の 1 バイト）
fn unfilter(rows: &mut [u8], stride: usize, bpp: usize) -> Result<(), PngError> {
    let n = rows.len() / (1 + stride);
    for y in 0..n {
        let (done, rest) = rows.split_at_mut(y * (1 + stride));
        let prev: &[u8] = if y == 0 { &[] } else { &done[(y - 1) * (1 + stride) + 1..] };
        let (kind, cur) = rest[..1 + stride].split_first_mut().ok_or(PngError::BadData)?;
        let up = |i: usize| if prev.is_empty() { 0 } else { prev[i] };
        match *kind {
            0 => {}
            1 => {
                for i in bpp..stride {
                    cur[i] = cur[i].wrapping_add(cur[i - bpp]);
                }
            }
            2 => {
                for (i, c) in cur.iter_mut().enumerate() {
                    *c = c.wrapping_add(up(i));
                }
            }
            3 => {
                for i in 0..stride {
                    let left = if i >= bpp { cur[i - bpp] } else { 0 };
                    cur[i] = cur[i].wrapping_add(((left as u16 + up(i) as u16) / 2) as u8);
                }
            }
            4 => {
                for i in 0..stride {
                    let a = if i >= bpp { cur[i - bpp] } else { 0 };
                    let c = if i >= bpp { up(i - bpp) } else { 0 };
                    cur[i] = cur[i].wrapping_add(paeth(a, up(i), c));
                }
            }
            _ => return Err(PngError::BadFilter),
        }
    }
    Ok(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// 行の中の `n` 番目の成分（ビットの深さが 8 未満なら左から詰めたもの、16 なら上位 1 バイト）。
/// 16 ビットのときは (上位, 下位) の下位も返す（tRNS の比べ方のため）
fn sample(row: &[u8], depth: u8, n: usize) -> (u16, u8) {
    match depth {
        16 => (u16::from_be_bytes([row[n * 2], row[n * 2 + 1]]), row[n * 2]),
        8 => (row[n] as u16, row[n]),
        d => {
            let bit = n * d as usize;
            let v = (row[bit / 8] >> (8 - d as usize - bit % 8)) & ((1u8 << d) - 1);
            // 0〜255 に広げる
            let scaled = (v as u16 * 255 / ((1u16 << d) - 1)) as u8;
            (v as u16, scaled)
        }
    }
}

/// 1 画素を 0x00RRGGBB に（アルファは `bg` の上に重ねる）
fn pixel(h: &Header, row: &[u8], x: usize, palette: &[[u8; 3]], trns: &[u8], bg: u32) -> Result<u32, PngError> {
    let ch = h.channels();
    let (r, g, b, a) = match h.color {
        0 => {
            let (raw, v) = sample(row, h.depth, x);
            let clear = trns.len() >= 2 && u16::from_be_bytes([trns[0], trns[1]]) == raw;
            (v, v, v, if clear { 0 } else { 255 })
        }
        2 => {
            let s: [(u16, u8); 3] = core::array::from_fn(|i| sample(row, h.depth, x * ch + i));
            let clear = trns.len() >= 6
                && (0..3).all(|i| u16::from_be_bytes([trns[i * 2], trns[i * 2 + 1]]) == s[i].0);
            (s[0].1, s[1].1, s[2].1, if clear { 0 } else { 255 })
        }
        3 => {
            let (i, _) = sample(row, h.depth, x);
            let c = palette.get(i as usize).ok_or(PngError::BadPalette)?;
            (c[0], c[1], c[2], trns.get(i as usize).copied().unwrap_or(255))
        }
        4 => {
            let v = sample(row, h.depth, x * 2).1;
            (v, v, v, sample(row, h.depth, x * 2 + 1).1)
        }
        _ => {
            let s: [u8; 4] = core::array::from_fn(|i| sample(row, h.depth, x * 4 + i).1);
            (s[0], s[1], s[2], s[3])
        }
    };
    let blend = |c: u8, shift: u32| {
        let back = (bg >> shift) & 0xFF;
        ((c as u32 * a as u32 + back * (255 - a as u32) + 127) / 255) << shift
    };
    Ok(blend(r, 16) | blend(g, 8) | blend(b, 0))
}
