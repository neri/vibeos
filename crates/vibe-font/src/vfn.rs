//! なめらかなフォントの形式 `.VFN`（ホストのツール `tools/vibe-fontgen` が TrueType から作る）。
//!
//! 決まった大きさで先にラスタライズした字を、1 ピクセル 4 ビットの濃さ（16 段階）で持つ。
//! ここは読むだけ（描くのは `vibe_gfx::canvas`）。すべてリトルエンディアン:
//!
//! | 位置 | 大きさ | 中身 |
//! |---|---|---|
//! | 0 | 4 | `"VFN1"` |
//! | 4 | 2 | 大きさ（ピクセル） |
//! | 6 | 2 | 行の高さ |
//! | 8 | 2 | ベースライン（行の上端から） |
//! | 10 | 2 | 字の数 n |
//! | 12 | 2 | 固定幅なら 1 文字の幅、プロポーショナルなら 0 |
//! | 14 | 2 | 0 |
//! | 16 | 16 × n | 字の表（文字コードの順）: 文字コード u32, 濃さの位置 u32（濃さの並びの先頭から）, 幅 u8, 高さ u8, 左の位置 i8（ペンから）, 上の位置 i8（ベースラインから、上が負）, 送り幅 u16（1/16 ピクセル）, 0 u16 |
//! | 16 + 16n | | 濃さの並び: 字ごとに 1 行 ceil(幅 / 2) バイト、下位 4 ビットが左のピクセル |
//!
//! 文字コードの表は疎なので、後から全角の字を足せる。

pub const MAGIC: &[u8; 4] = b"VFN1";
pub const HEADER_SIZE: usize = 16;
pub const ENTRY_SIZE: usize = 16;
/// 送り幅の単位（1 ピクセルをこの数に分ける）
pub const SUBPIXELS: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfnError {
    TooShort,
    BadMagic,
    BadGlyph,
}

#[derive(Clone, Copy)]
pub struct VFont<'a> {
    data: &'a [u8],
    pub size: u16,
    pub line_height: u16,
    pub ascent: u16,
    /// 固定幅なら 1 文字の幅（ピクセル）、プロポーショナルなら 0
    pub cell_width: u16,
    count: usize,
}

/// 1 つの字
#[derive(Clone, Copy, Debug)]
pub struct Glyph<'a> {
    pub width: u8,
    pub height: u8,
    /// ペンの位置から字の左端まで
    pub left: i8,
    /// ベースラインから字の上端まで（上が負）
    pub top: i8,
    /// 次の字までの送り幅（1/16 ピクセル）
    pub advance: u16,
    bits: &'a [u8],
}

impl Glyph<'_> {
    /// (x, y) の濃さ（0〜255）
    pub fn alpha(&self, x: usize, y: usize) -> u8 {
        if x >= self.width as usize || y >= self.height as usize {
            return 0;
        }
        let stride = (self.width as usize).div_ceil(2);
        let b = self.bits[y * stride + x / 2];
        let v = if x % 2 == 0 { b & 0x0F } else { b >> 4 };
        v * 17
    }
}

fn u16_at(d: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([d[i], d[i + 1]])
}

fn u32_at(d: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]])
}

impl<'a> VFont<'a> {
    /// 形を確かめて読む（字の濃さがデータの外を指していれば誤り）
    pub fn parse(data: &'a [u8]) -> Result<VFont<'a>, VfnError> {
        if data.len() < HEADER_SIZE {
            return Err(VfnError::TooShort);
        }
        if &data[..4] != MAGIC {
            return Err(VfnError::BadMagic);
        }
        let count = u16_at(data, 10) as usize;
        let bits_start = HEADER_SIZE + count * ENTRY_SIZE;
        if data.len() < bits_start {
            return Err(VfnError::TooShort);
        }
        let font = VFont {
            data,
            size: u16_at(data, 4),
            line_height: u16_at(data, 6),
            ascent: u16_at(data, 8),
            cell_width: u16_at(data, 12),
            count,
        };
        let mut prev = None;
        for i in 0..count {
            let e = HEADER_SIZE + i * ENTRY_SIZE;
            let code = u32_at(data, e);
            if prev.is_some_and(|p| p >= code) {
                return Err(VfnError::BadGlyph); // 文字コードの順に並んでいない
            }
            prev = Some(code);
            let (w, h) = (data[e + 8] as usize, data[e + 9] as usize);
            let start = bits_start + u32_at(data, e + 4) as usize;
            if start + w.div_ceil(2) * h > data.len() {
                return Err(VfnError::BadGlyph);
            }
        }
        Ok(font)
    }

    pub fn glyph_count(&self) -> usize {
        self.count
    }

    /// 文字 `c` の字（無ければ None）
    pub fn glyph(&self, c: char) -> Option<Glyph<'a>> {
        let code = c as u32;
        let (mut lo, mut hi) = (0, self.count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let e = HEADER_SIZE + mid * ENTRY_SIZE;
            let m = u32_at(self.data, e);
            if m == code {
                let d = self.data;
                let bits_start = HEADER_SIZE + self.count * ENTRY_SIZE;
                let (w, h) = (d[e + 8], d[e + 9]);
                let start = bits_start + u32_at(d, e + 4) as usize;
                let len = (w as usize).div_ceil(2) * h as usize;
                return Some(Glyph {
                    width: w,
                    height: h,
                    left: d[e + 10] as i8,
                    top: d[e + 11] as i8,
                    advance: u16_at(d, e + 12),
                    bits: &d[start..start + len],
                });
            }
            if m < code {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
    }

    /// 表示に使う字（無ければ '?'、それも無ければ None）
    pub fn glyph_or_replacement(&self, c: char) -> Option<Glyph<'a>> {
        self.glyph(c).or_else(|| self.glyph('?'))
    }

    /// 文字列の幅（ピクセル。描くときと同じく、ペンを 1/16 ピクセルで進めて最後に丸める）
    pub fn measure(&self, text: &str) -> i32 {
        let mut pen = 0i32;
        for c in text.chars() {
            if let Some(g) = self.glyph_or_replacement(c) {
                pen += g.advance as i32;
            }
        }
        (pen + SUBPIXELS / 2) / SUBPIXELS
    }

    /// 幅 `max` に入る先頭の文字数
    pub fn fit(&self, text: &str, max: i32) -> usize {
        let mut pen = 0i32;
        for (i, c) in text.chars().enumerate() {
            if let Some(g) = self.glyph_or_replacement(c) {
                pen += g.advance as i32;
            }
            if (pen + SUBPIXELS / 2) / SUBPIXELS > max {
                return i;
            }
        }
        text.chars().count()
    }
}

/// `.VFN` を組み立てる（ホストのツールとテスト用）
#[cfg(any(test, feature = "std"))]
pub mod build {
    extern crate std;
    use std::vec::Vec;

    pub struct GlyphData {
        pub code: u32,
        pub width: u8,
        pub height: u8,
        pub left: i8,
        pub top: i8,
        pub advance: u16,
        /// 幅 × 高さの濃さ（0〜15）
        pub alpha: Vec<u8>,
    }

    pub fn encode(size: u16, line_height: u16, ascent: u16, cell_width: u16, glyphs: &mut [GlyphData]) -> Vec<u8> {
        glyphs.sort_by_key(|g| g.code);
        let mut out = Vec::new();
        out.extend_from_slice(super::MAGIC);
        for v in [size, line_height, ascent, glyphs.len() as u16, cell_width, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        let mut bits = Vec::new();
        for g in glyphs.iter() {
            out.extend_from_slice(&g.code.to_le_bytes());
            out.extend_from_slice(&(bits.len() as u32).to_le_bytes());
            out.extend_from_slice(&[g.width, g.height, g.left as u8, g.top as u8]);
            out.extend_from_slice(&g.advance.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            for row in g.alpha.chunks(g.width.max(1) as usize).take(g.height as usize) {
                for pair in row.chunks(2) {
                    bits.push((pair[0] & 0x0F) | pair.get(1).map_or(0, |v| (v & 0x0F) << 4));
                }
            }
        }
        out.extend_from_slice(&bits);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::{GlyphData, encode};
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn sample() -> Vec<u8> {
        let mut glyphs = vec![
            GlyphData { code: 'B' as u32, width: 3, height: 2, left: 1, top: -2, advance: 72, alpha: vec![15, 0, 7, 1, 2, 3] },
            GlyphData { code: '?' as u32, width: 1, height: 1, left: 0, top: -1, advance: 32, alpha: vec![9] },
            GlyphData { code: ' ' as u32, width: 0, height: 0, left: 0, top: 0, advance: 40, alpha: vec![] },
        ];
        encode(10, 12, 9, 0, &mut glyphs)
    }

    #[test]
    fn round_trip() {
        let d = sample();
        let f = VFont::parse(&d).unwrap();
        assert_eq!((f.size, f.line_height, f.ascent, f.cell_width), (10, 12, 9, 0));
        assert_eq!(f.glyph_count(), 3);
        let b = f.glyph('B').unwrap();
        assert_eq!((b.width, b.height, b.left, b.top, b.advance), (3, 2, 1, -2, 72));
        assert_eq!([b.alpha(0, 0), b.alpha(1, 0), b.alpha(2, 0)], [255, 0, 119]);
        assert_eq!([b.alpha(0, 1), b.alpha(1, 1), b.alpha(2, 1)], [17, 34, 51]);
        assert_eq!(b.alpha(3, 0), 0);
        assert!(f.glyph('A').is_none());
        assert_eq!(f.glyph_or_replacement('A').unwrap().advance, 32);
        assert_eq!(f.glyph(' ').unwrap().width, 0);
    }

    #[test]
    fn measure_rounds_once() {
        let d = sample();
        let f = VFont::parse(&d).unwrap();
        // 72 + 40 + 72 = 184 / 16 = 11.5 → 12
        assert_eq!(f.measure("B B"), 12);
        assert_eq!(f.measure(""), 0);
        assert_eq!(f.fit("B B", 5), 1);
        assert_eq!(f.fit("B B", 12), 3);
    }

    #[test]
    fn rejects_broken_data() {
        let d = sample();
        assert_eq!(VFont::parse(&d[..10]).err(), Some(VfnError::TooShort));
        let mut bad = d.clone();
        bad[0] = b'X';
        assert_eq!(VFont::parse(&bad).err(), Some(VfnError::BadMagic));
        // 濃さの並びを切る
        assert_eq!(VFont::parse(&d[..d.len() - 1]).err(), Some(VfnError::BadGlyph));
        // 字の数を大きくする
        let mut bad = d.clone();
        bad[10] = 200;
        assert_eq!(VFont::parse(&bad).err(), Some(VfnError::TooShort));
        // 順番を壊す
        let mut bad = d;
        bad[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&('Z' as u32).to_le_bytes());
        assert_eq!(VFont::parse(&bad).err(), Some(VfnError::BadGlyph));
    }
}
