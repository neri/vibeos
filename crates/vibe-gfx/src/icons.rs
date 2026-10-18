//! アイコンをまとめたファイル `.VIA`（ホストのツール `tools/vibe-icongen` が手で描いた白黒の絵から作る）。
//!
//! 16x16 の色番号（`palette::VGA16` の番号、透明は `TRANSPARENT`。今の絵は黒と白だけ）。すべてリトルエンディアン:
//!
//! | 位置 | 大きさ | 中身 |
//! |---|---|---|
//! | 0 | 4 | `"VIA4"` |
//! | 4 | 2 | 1 辺のピクセル数 s |
//! | 6 | 2 | アイコンの数 n |
//! | 8 | (12 + s²) × n | 名前（ASCII、12 バイトに足りない分は 0）と、s × s の色番号（1 画素 1 バイト）。名前の順 |

use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: &[u8; 4] = b"VIA4";
pub const NAME_LEN: usize = 12;
/// 透明な画素
pub const TRANSPARENT: u8 = 0xFF;
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
        if data.len() < HEADER_SIZE + count * (NAME_LEN + size * size) {
            return Err(ViaError::TooShort);
        }
        Ok(IconSet { data, size, count })
    }

    fn entry(&self, i: usize) -> (&'a str, &'a [u8]) {
        let len = NAME_LEN + self.size * self.size;
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

    /// 名前 `name` のアイコンのピクセル（s × s の色番号、透明は `TRANSPARENT`）
    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        (0..self.count).map(|i| self.entry(i)).find(|(n, _)| *n == name).map(|(_, px)| px)
    }
}

/// 手で描いた白黒の絵（1 行 `size` 文字 × `size` 行。`#` は黒、`.` は白、空白は透明）を色番号にする。
/// 行の数・長さが違う、ほかの文字があるときは、その行の番号（1 から）を添えて Err
pub fn parse_art(text: &str, size: usize) -> Result<Vec<u8>, String> {
    use crate::palette::{BLACK, WHITE};
    use alloc::format;
    let rows: Vec<&str> = text.lines().collect();
    if rows.len() != size {
        return Err(format!("{} rows, expected {size}", rows.len()));
    }
    let mut px = Vec::with_capacity(size * size);
    for (n, row) in rows.iter().enumerate() {
        if row.chars().count() != size {
            return Err(format!("row {}: {} columns, expected {size}", n + 1, row.chars().count()));
        }
        for c in row.chars() {
            px.push(match c {
                '#' => BLACK,
                '.' => WHITE,
                ' ' => TRANSPARENT,
                _ => return Err(format!("row {}: unexpected {c:?}", n + 1)),
            });
        }
    }
    Ok(px)
}

/// `.VIA` を組み立てる（名前は 12 バイトまで。ホストのツールとテスト用）
pub fn encode(size: u16, icons: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn round_trip() {
        let icons = vec![("app".to_string(), vec![1, 2, 3, TRANSPARENT]), ("calc".to_string(), vec![15; 4])];
        let data = encode(2, &icons);
        assert_eq!(data.len(), 8 + 2 * (NAME_LEN + 4));
        let set = IconSet::parse(&data).unwrap();
        assert_eq!((set.size, set.len()), (2, 2));
        assert_eq!(set.names().collect::<Vec<_>>(), ["app", "calc"]);
        assert_eq!(set.get("app"), Some(&[1u8, 2, 3, TRANSPARENT][..]));
        assert!(set.get("nothing").is_none());
        assert_eq!(IconSet::parse(&data[..data.len() - 1]).err(), Some(ViaError::TooShort));
        assert_eq!(IconSet::parse(b"VIA1\x02\x00\x00\x00").err(), Some(ViaError::BadMagic));
    }

    #[test]
    fn parse_art_reads_black_white_and_clear() {
        use crate::palette::{BLACK, WHITE};
        assert_eq!(parse_art("#.\n .\n", 2).unwrap(), [BLACK, WHITE, TRANSPARENT, WHITE]);
        assert!(parse_art("#.\n", 2).unwrap_err().contains("1 rows"));
        assert!(parse_art("#.\n#\n", 2).unwrap_err().contains("row 2"));
        assert!(parse_art("#x\n..\n", 2).unwrap_err().contains("'x'"));
    }
}
