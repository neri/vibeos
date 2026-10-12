//! 展開器
//!
//! 出力先の `&mut [u8]` に直接書く。一致は「すでに書いた出力」から 1 バイトずつ写す
//! （コピー元とコピー先が重なってよい）。壊れたデータでも panic せず [`Error`] を返す。

use crate::format::*;
use crate::{Error, Version};

/// 圧縮データを前から読む。LZSA2 のニブルは「必要になったときに 1 バイト読み、上位 4 ビットを
/// 返して下位 4 ビットを次のために取っておく」
struct Reader<'a> {
    src: &'a [u8],
    pos: usize,
    nibble: Option<u8>,
}

impl<'a> Reader<'a> {
    fn new(src: &'a [u8]) -> Self {
        Reader { src, pos: 0, nibble: None }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let b = *self.src.get(self.pos).ok_or(Error::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn u16(&mut self) -> Result<usize, Error> {
        let lo = self.byte()? as usize;
        let hi = self.byte()? as usize;
        Ok(lo | hi << 8)
    }

    fn nibble(&mut self) -> Result<usize, Error> {
        match self.nibble.take() {
            Some(n) => Ok(n as usize),
            None => {
                let b = self.byte()?;
                self.nibble = Some(b & 0x0f);
                Ok((b >> 4) as usize)
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Truncated)?;
        let s = self.src.get(self.pos..end).ok_or(Error::Truncated)?;
        self.pos = end;
        Ok(s)
    }
}

/// リテラルを `out[*p..]` に写す
fn put_literals(out: &mut [u8], p: &mut usize, lits: &[u8]) -> Result<(), Error> {
    let dst = out.get_mut(*p..*p + lits.len()).ok_or(Error::OutputFull)?;
    dst.copy_from_slice(lits);
    *p += lits.len();
    Ok(())
}

/// `dist` バイト前から `len` バイトを `out[*p..]` に写す（重なってよいので 1 バイトずつ）
fn put_match(out: &mut [u8], p: &mut usize, dist: usize, len: usize) -> Result<(), Error> {
    if dist == 0 || dist > *p {
        return Err(Error::BadOffset);
    }
    if out.len() - *p < len {
        return Err(Error::OutputFull);
    }
    for i in *p..*p + len {
        out[i] = out[i - dist];
    }
    *p += len;
    Ok(())
}

/// ブロックを 1 つ展開する。`out[..start]` はそれまでに展開したもの（一致で参照してよい）で、
/// `out[start..]` に書く。書き終えた位置を返す
///
/// `raw` のときは EOD の印で終わる（raw block）。そうでなければ入力の終わりで終わる
/// （ストリームのフレームの中身。最後のコマンドはリテラルだけ）
pub fn decompress_block(
    version: Version,
    src: &[u8],
    out: &mut [u8],
    start: usize,
    raw: bool,
) -> Result<usize, Error> {
    if start > out.len() {
        return Err(Error::OutputFull);
    }
    let mut r = Reader::new(src);
    let mut p = start;
    match version {
        Version::V1 => loop {
            if r.at_end() {
                return if raw { Err(Error::Truncated) } else { Ok(p) };
            }
            let token = r.byte()?;
            let mut lits = ((token >> 4) & 7) as usize;
            if lits == V1_LIT_RUN {
                lits = match r.byte()? {
                    250 => 256 + r.byte()? as usize,
                    249 => r.u16()?,
                    b => V1_LIT_RUN + b as usize,
                };
            }
            put_literals(out, &mut p, r.take(lits)?)?;
            if !raw && r.at_end() {
                return Ok(p);
            }
            let mut dist = (r.byte()? ^ 0xff) as usize;
            if token & V1_LONG_OFFSET != 0 {
                dist |= ((r.byte()? ^ 0xff) as usize) << 8;
            }
            dist += 1;
            let mut len = (token & 0x0f) as usize;
            if len == V1_MATCH_RUN {
                len = match r.byte()? {
                    239 => 256 + r.byte()? as usize,
                    238 => r.u16()?,
                    b => MIN_MATCH_V1 + V1_MATCH_RUN + b as usize,
                };
                if len == 0 {
                    return Ok(p); // EOD
                }
            } else {
                len += MIN_MATCH_V1;
            }
            put_match(out, &mut p, dist, len)?;
        },
        Version::V2 => {
            // 前回のオフセット。ブロックごとに 0 から始まる
            let mut rep = 0usize;
            loop {
                if r.at_end() {
                    return if raw { Err(Error::Truncated) } else { Ok(p) };
                }
                let token = r.byte()?;
                let mut lits = ((token >> 3) & 3) as usize;
                if lits == V2_LIT_RUN {
                    lits = v2_len(&mut r, V2_LIT_RUN)?;
                }
                put_literals(out, &mut p, r.take(lits)?)?;
                if !raw && r.at_end() {
                    return Ok(p);
                }
                let z = ((token >> 5) & 1) as usize;
                let dist = match token & 0xe0 {
                    0x00 | 0x20 => (((r.nibble()? << 1) | z) ^ 0x1e) + 1,
                    0x40 | 0x60 => ((r.byte()? as usize | z << 8) ^ 0x0ff) + 1,
                    0x80 | 0xa0 => {
                        let hi = r.nibble()?;
                        let lo = r.byte()? as usize;
                        ((lo | hi << 9 | z << 8) ^ 0x1eff) + 512 + 1
                    }
                    V2_REP => rep,
                    _ => {
                        let hi = r.byte()? as usize;
                        let lo = r.byte()? as usize;
                        ((hi << 8 | lo) ^ 0xffff) + 1
                    }
                };
                let mut len = (token & 7) as usize;
                if len == V2_MATCH_RUN {
                    len = v2_len(&mut r, MIN_MATCH_V2 + V2_MATCH_RUN)?;
                    if len == 0 {
                        return Ok(p); // EOD
                    }
                } else {
                    len += MIN_MATCH_V2;
                }
                put_match(out, &mut p, dist, len)?;
                rep = dist;
            }
        }
    }
}

/// LZSA2 の長さの続き（ニブル → 1 バイト → 2 バイト）。`base` はトークンの欄が埋まったときの値。
/// 1 バイトの値が 256 になる組み合わせ（一致の長さで 232）は EOD で、0 を返す
fn v2_len(r: &mut Reader, base: usize) -> Result<usize, Error> {
    let n = r.nibble()?;
    let mut len = base + n;
    if n == 15 {
        len += r.byte()? as usize;
        if len == 257 {
            len = r.u16()?;
        } else if len == 256 {
            len = 0;
        }
    }
    Ok(len)
}

/// raw block を展開する。書いた大きさを返す。空の入力は空のデータ（本家の `lzsa -r` は
/// 空のファイルを空のファイルに圧縮する）
pub fn decompress_raw(version: Version, src: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    if src.is_empty() {
        return Ok(0);
    }
    decompress_block(version, src, out, 0, true)
}

/// ストリームのヘッダを読み、ブロックの形式を返す
fn header(src: &[u8]) -> Result<Version, Error> {
    if src.len() < 3 || src[..2] != MAGIC {
        return Err(Error::BadHeader);
    }
    match src[2] {
        0x00 => Ok(Version::V1),
        0x20 => Ok(Version::V2),
        _ => Err(Error::BadHeader),
    }
}

/// フレームの長さの欄を読む。`(大きさ, 無圧縮か)`。終わりの印なら `None`
fn frame_header(r: &mut Reader) -> Result<Option<(usize, bool)>, Error> {
    let h = r.take(3)?;
    if h[2] & 0x7e != 0 {
        return Err(Error::BadFrame);
    }
    let size = h[0] as usize | (h[1] as usize) << 8 | ((h[2] & 1) as usize) << 16;
    let uncompressed = h[2] & FRAME_UNCOMPRESSED != 0;
    if size == 0 && !uncompressed {
        return Ok(None);
    }
    Ok(Some((size, uncompressed)))
}

/// ストリーム形式を展開する。書いた大きさを返す
pub fn decompress(src: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    let version = header(src)?;
    let mut r = Reader::new(&src[3..]);
    let mut p = 0;
    while let Some((size, uncompressed)) = frame_header(&mut r)? {
        let data = r.take(size)?;
        if uncompressed {
            put_literals(out, &mut p, data)?;
        } else {
            p = decompress_block(version, data, out, p, false)?;
        }
    }
    Ok(p)
}

/// ストリーム形式を展開して `Vec` で返す（展開後の大きさが分からなくてよい）
#[cfg(feature = "alloc")]
pub fn decompress_to_vec(src: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
    let version = header(src)?;
    let mut r = Reader::new(&src[3..]);
    let mut out = alloc::vec::Vec::new();
    while let Some((size, uncompressed)) = frame_header(&mut r)? {
        let data = r.take(size)?;
        if uncompressed {
            out.extend_from_slice(data);
        } else {
            let p = out.len();
            out.resize(p + crate::BLOCK_SIZE, 0);
            let end = decompress_block(version, data, &mut out, p, false)?;
            out.truncate(end);
        }
    }
    Ok(out)
}

/// raw block を展開して `Vec` で返す
#[cfg(feature = "alloc")]
pub fn decompress_raw_to_vec(version: Version, src: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
    let mut out = alloc::vec![0; crate::BLOCK_SIZE];
    let n = decompress_raw(version, src, &mut out)?;
    out.truncate(n);
    Ok(out)
}
