//! ビットマップフォント（なめらかなフォント `.VFN` は `vfn`）。
//!
//! 文字の大きさは `Font` から取る（呼び出し側で 8×8 を決め打ちしない）。
//! 1 行分のビットは `row_bits` で取り出し、bit 0 が左端のピクセル。

#![cfg_attr(not(test), no_std)]

pub mod vfn;
pub use vfn::{Glyph, VFont};

pub struct Font {
    pub width: usize,
    pub height: usize,
    /// ASCII 0x00〜0x7F のグリフ（1 バイト = 1 行, bit 0 = 左端）
    glyphs: &'static [[u8; 8]; 128],
    /// 表示できない文字の代わりに使う文字
    replacement: char,
}

/// 8×8 の ASCII フォント（データは font8x8 クレート）
pub static FONT_8X8: Font = Font {
    width: 8,
    height: 8,
    glyphs: &font8x8::legacy::BASIC_LEGACY,
    replacement: '?',
};

impl Font {
    /// この文字を表示できるか
    pub fn has_glyph(&self, c: char) -> bool {
        (c as u32) < 128 && (c == ' ' || self.glyphs[c as usize] != [0; 8])
    }

    /// 文字 `c` の `y` 行目のビット（bit 0 が左端）。表示できない文字は代替文字になる。
    pub fn row_bits(&self, c: char, y: usize) -> u32 {
        let c = if self.has_glyph(c) { c } else { self.replacement };
        self.glyphs[c as usize].get(y).copied().unwrap_or(0) as u32
    }

    /// 文字 `c` の (x, y) のピクセルが点灯しているか
    pub fn pixel(&self, c: char, x: usize, y: usize) -> bool {
        x < self.width && (self.row_bits(c, y) >> x) & 1 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(c: char) -> Vec<String> {
        let f = &FONT_8X8;
        (0..f.height)
            .map(|y| (0..f.width).map(|x| if f.pixel(c, x, y) { '#' } else { '.' }).collect())
            .collect()
    }

    #[test]
    fn letter_a_shape() {
        assert_eq!(
            render('A'),
            [
                "..##....", ".####...", "##..##..", "##..##..", "######..", "##..##..", "##..##..", "........",
            ]
        );
    }

    #[test]
    fn space_is_blank_but_printable() {
        assert!(FONT_8X8.has_glyph(' '));
        assert!(render(' ').iter().all(|r| r == "........"));
    }

    #[test]
    fn unknown_chars_use_replacement() {
        assert!(!FONT_8X8.has_glyph('あ'));
        assert_eq!(render('あ'), render('?'));
        assert_eq!(render('\u{1}'), render('?'));
    }
}
