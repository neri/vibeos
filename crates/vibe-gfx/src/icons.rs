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

use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: &[u8; 4] = b"VIA1";
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
}

impl<'a> IconSet<'a> {
    pub fn parse(data: &'a [u8]) -> Result<IconSet<'a>, ViaError> {
        if data.len() < HEADER_SIZE {
            return Err(ViaError::TooShort);
        }
        if &data[..4] != MAGIC {
            return Err(ViaError::BadMagic);
        }
        let size = u16::from_le_bytes([data[4], data[5]]) as usize;
        let count = u16::from_le_bytes([data[6], data[7]]) as usize;
        if data.len() < HEADER_SIZE + count * (NAME_LEN + 4 * size * size) {
            return Err(ViaError::TooShort);
        }
        Ok(IconSet { data, size, count })
    }

    fn entry(&self, i: usize) -> (&'a str, &'a [u8]) {
        let len = NAME_LEN + 4 * self.size * self.size;
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

    /// 名前 `name` のアイコンのピクセル（s × s の 0xAARRGGBB）
    pub fn get(&self, name: &str) -> Option<Vec<u32>> {
        (0..self.count).map(|i| self.entry(i)).find(|(n, _)| *n == name).map(|(_, px)| {
            px.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
        })
    }
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
    }
}
