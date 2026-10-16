//! アイコンをまとめたファイル `.VIA`（ホストのツール `tools/vibe-icongen` が手で書いた SVG から作る）。
//!
//! すべてリトルエンディアン:
//!
//! | 位置 | 大きさ | 中身 |
//! |---|---|---|
//! | 0 | 4 | `"VIA1"` |
//! | 4 | 2 | 1 辺のピクセル数 s |
//! | 6 | 2 | アイコンの数 n |
//! | 8 | (12 + 4s²) × n | 名前（ASCII、12 バイトに足りない分は 0）と、s × s の 0xAARRGGBB（色にアルファを掛けたもの）。名前の順 |
//!
//! 16 色のもの（モード 12h の画面用。`"VIA4"`）は、画素が 1 バイト（`palette::VGA16` の番号、透明は
//! `TRANSPARENT`）で、1 つのアイコンは 12 + s² バイト。ほかは同じ

use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: &[u8; 4] = b"VIA1";
pub const MAGIC16: &[u8; 4] = b"VIA4";
/// 16 色のアイコンの透明な画素
pub const TRANSPARENT: u8 = 0xFF;
pub const NAME_LEN: usize = 12;
const HEADER_SIZE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViaError {
    TooShort,
    BadMagic,
}

#[derive(Clone, Copy)]
pub struct IconSet<'a> {
    data: &'a [u8],
    pub size: usize,
    count: usize,
    /// 1 画素のバイト数（`VIA1` は 4、`VIA4` は 1）
    px_bytes: usize,
}

impl<'a> IconSet<'a> {
    pub fn parse(data: &'a [u8]) -> Result<IconSet<'a>, ViaError> {
        if data.len() < HEADER_SIZE {
            return Err(ViaError::TooShort);
        }
        let px_bytes = match &data[..4] {
            m if m == MAGIC => 4,
            m if m == MAGIC16 => 1,
            _ => return Err(ViaError::BadMagic),
        };
        let size = u16::from_le_bytes([data[4], data[5]]) as usize;
        let count = u16::from_le_bytes([data[6], data[7]]) as usize;
        if data.len() < HEADER_SIZE + count * (NAME_LEN + px_bytes * size * size) {
            return Err(ViaError::TooShort);
        }
        Ok(IconSet { data, size, count, px_bytes })
    }

    /// 16 色のアイコン（`VIA4`）か
    pub fn is_vga16(&self) -> bool {
        self.px_bytes == 1
    }

    fn entry(&self, i: usize) -> (&'a str, &'a [u8]) {
        let len = NAME_LEN + self.px_bytes * self.size * self.size;
        let e = &self.data[HEADER_SIZE + i * len..HEADER_SIZE + (i + 1) * len];
        let name = &e[..NAME_LEN];
        let end = name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        (core::str::from_utf8(&name[..end]).unwrap_or(""), &e[NAME_LEN..])
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 名前の一覧
    pub fn names(&self) -> impl Iterator<Item = &'a str> + '_ {
        (0..self.count).map(|i| self.entry(i).0)
    }

    /// 名前 `name` のアイコンのピクセル（s × s の 0xAARRGGBB）。16 色のファイルなら None
    pub fn get(&self, name: &str) -> Option<Vec<u32>> {
        if self.is_vga16() {
            return None;
        }
        (0..self.count).map(|i| self.entry(i)).find(|(n, _)| *n == name).map(|(_, px)| {
            px.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
        })
    }

    /// 名前 `name` の 16 色のアイコンのピクセル（s × s の色番号、透明は `TRANSPARENT`）。16 色のファイルでなければ None
    pub fn get16(&self, name: &str) -> Option<&'a [u8]> {
        if !self.is_vga16() {
            return None;
        }
        (0..self.count).map(|i| self.entry(i)).find(|(n, _)| *n == name).map(|(_, px)| px)
    }
}

/// 成分の差がこれ以下の色は灰色とみなす（8bit）
const GRAYISH: u8 = 48;

/// アルファ付きのアイコン（事前に掛けたアルファの 0xAARRGGBB）を 16 色にする: アルファが半分より小さい画素は
/// 透明、ほかはアルファを戻した色に一番近い 16 色。灰色に近い色（青みのある暗い灰など）は黒・灰・白の中から
/// 選ぶ（そうしないと縁が紺や青緑の点になる）。誤差拡散はしない（小さな絵では点がざらつくため）
pub fn quantize16(px: &[u32]) -> Vec<u8> {
    use crate::palette::{BLACK, DARK_GRAY, LIGHT_GRAY, VGA16, WHITE, nearest16};
    px.iter()
        .map(|&p| {
            let a = p >> 24;
            if a < 128 {
                return TRANSPARENT;
            }
            let c: [u8; 3] = core::array::from_fn(|i| (((p >> (16 - 8 * i)) & 0xFF) * 255 / a).min(255) as u8);
            let (lo, hi) = (c.iter().min().copied().unwrap_or(0), c.iter().max().copied().unwrap_or(0));
            if hi - lo <= GRAYISH {
                let v = (c[0] as i32 * 3 + c[1] as i32 * 4 + c[2] as i32 * 2) / 9 >> 2;
                return *[BLACK, DARK_GRAY, LIGHT_GRAY, WHITE]
                    .iter()
                    .min_by_key(|&&g| (VGA16[g as usize][0] as i32 - v).abs())
                    .unwrap_or(&BLACK);
            }
            nearest16(c.map(|v| v >> 2))
        })
        .collect()
}

/// 16 色の `.VIA`（`VIA4`）を組み立てる（ホストのツールとテスト用）
pub fn encode16(size: u16, icons: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC16);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(icons.len() as u16).to_le_bytes());
    for (name, px) in icons {
        assert!(name.len() <= NAME_LEN && px.len() == size as usize * size as usize, "bad icon {name}");
        let mut n = [0u8; NAME_LEN];
        n[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&n);
        out.extend_from_slice(px);
    }
    out
}

/// `.VIA` を組み立てる（名前は 12 バイトまで。ホストのツールとテスト用）
pub fn encode(size: u16, icons: &[(String, Vec<u32>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(icons.len() as u16).to_le_bytes());
    for (name, px) in icons {
        assert!(name.len() <= NAME_LEN && px.len() == size as usize * size as usize, "bad icon {name}");
        let mut n = [0u8; NAME_LEN];
        n[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&n);
        for p in px {
            out.extend_from_slice(&p.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn round_trip() {
        let icons = vec![("calc".to_string(), vec![0xFF00_00FFu32; 4]), ("folder".to_string(), vec![0x8000_4000; 4])];
        let data = encode(2, &icons);
        let set = IconSet::parse(&data).unwrap();
        assert_eq!((set.size, set.len()), (2, 2));
        assert_eq!(set.names().collect::<Vec<_>>(), ["calc", "folder"]);
        assert_eq!(set.get("folder").unwrap(), vec![0x8000_4000; 4]);
        assert!(set.get("nothing").is_none());
        assert_eq!(IconSet::parse(&data[..data.len() - 1]).err(), Some(ViaError::TooShort));
        assert_eq!(IconSet::parse(b"VIAX\x02\x00\x00\x00").err(), Some(ViaError::BadMagic));
        assert!(!set.is_vga16() && set.get16("calc").is_none());
    }

    #[test]
    fn vga16_round_trip() {
        let icons = vec![("app".to_string(), vec![1, 2, 3, TRANSPARENT]), ("calc".to_string(), vec![15; 4])];
        let data = encode16(2, &icons);
        assert_eq!(data.len(), 8 + 2 * (NAME_LEN + 4));
        let set = IconSet::parse(&data).unwrap();
        assert!(set.is_vga16());
        assert_eq!(set.get16("app"), Some(&[1u8, 2, 3, TRANSPARENT][..]));
        assert!(set.get("app").is_none() && set.get16("nothing").is_none());
        assert_eq!(IconSet::parse(&data[..data.len() - 1]).err(), Some(ViaError::TooShort));
    }

    #[test]
    fn quantize16_thresholds_alpha_and_finds_colors() {
        use crate::palette::{BLUE, LIGHT_GRAY, WHITE};
        // 透明・半分より薄い・白・半分の濃さの紺（事前に掛けたアルファ）・明るい灰
        let px = [0x0000_0000, 0x7F7F_7F7F, 0xFFFF_FFFF, 0x8000_0040, 0xFFC0_C0C0];
        assert_eq!(quantize16(&px), [TRANSPARENT, TRANSPARENT, WHITE, BLUE, LIGHT_GRAY]);
        // 青みのある暗い灰は紺ではなく黒、青みのある明るい灰は明るい灰
        assert_eq!(quantize16(&[0xFF28_3046, 0xFF9C_A8BC]), [crate::palette::BLACK, LIGHT_GRAY]);
    }
}
