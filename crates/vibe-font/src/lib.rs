//! ビットマップフォント（8×8・それを縦に 2 倍した 8×16・VGA の BIOS の 8×16）。
//!
//! 文字の大きさは `Font` から取る（呼び出し側で 8×8 を決め打ちしない）。
//! 1 行分のビットは `row_bits` で取り出し、bit 0 が左端のピクセル。

#![cfg_attr(not(test), no_std)]

#[derive(Clone)]
pub struct Font {
    pub width: usize,
    pub height: usize,
    glyphs: Glyphs,
    /// 表示できない文字の代わりに使う文字
    replacement: char,
}

#[derive(Clone, Copy)]
enum Glyphs {
    /// ASCII 0x00〜0x7F の 8×8 のグリフ（1 バイト = 1 行, bit 0 = 左端。font8x8）
    Rows8(&'static [[u8; 8]; 128]),
    /// `Rows8` の各行を 2 回ずつ使って 8×16 にしたもの（8×16 のフォントが無いときの代わり）
    Rows8Tall(&'static [[u8; 8]; 128]),
    /// 256 字の 8×16 のグリフ（1 字 16 バイト、1 バイト = 1 行, bit 7 = 左端。VGA の BIOS の並び）
    Vga16(&'static [u8; 256 * 16]),
}

/// 8×8 の ASCII フォント（データは font8x8 クレート）
pub static FONT_8X8: Font = Font {
    width: 8,
    height: 8,
    glyphs: Glyphs::Rows8(&font8x8::legacy::BASIC_LEGACY),
    replacement: '?',
};

/// 8×8 のフォントを縦に 2 倍にした 8×16（VGA の BIOS の 8×16 のフォントが無いとき）
pub static FONT_8X16_TALL: Font = Font {
    width: 8,
    height: 16,
    glyphs: Glyphs::Rows8Tall(&font8x8::legacy::BASIC_LEGACY),
    replacement: '?',
};

impl Font {
    /// VGA の BIOS の 8×16 のフォント（256 字 × 16 バイト、bit 7 が左端）。使うのは ASCII の
    /// 表示できる字（0x20〜0x7E）だけ（ほかはコードページ 437 の絵なので代替文字にする）
    pub const fn vga16(data: &'static [u8; 256 * 16]) -> Font {
        Font { width: 8, height: 16, glyphs: Glyphs::Vga16(data), replacement: '?' }
    }

    /// この文字を表示できるか
    pub fn has_glyph(&self, c: char) -> bool {
        match self.glyphs {
            Glyphs::Rows8(g) | Glyphs::Rows8Tall(g) => (c as u32) < 128 && (c == ' ' || g[c as usize] != [0; 8]),
            Glyphs::Vga16(_) => (' '..='~').contains(&c),
        }
    }

    /// 文字 `c` の `y` 行目のビット（bit 0 が左端）。表示できない文字は代替文字になる。
    pub fn row_bits(&self, c: char, y: usize) -> u32 {
        let c = if self.has_glyph(c) { c } else { self.replacement } as usize;
        match self.glyphs {
            Glyphs::Rows8(g) => g[c].get(y).copied().unwrap_or(0) as u32,
            Glyphs::Rows8Tall(g) => g[c].get(y / 2).copied().unwrap_or(0) as u32,
            Glyphs::Vga16(d) => {
                if y < 16 {
                    d[c * 16 + y].reverse_bits() as u32
                } else {
                    0
                }
            }
        }
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
    fn tall_font_doubles_each_row() {
        let f = &FONT_8X16_TALL;
        assert_eq!((f.width, f.height), (8, 16));
        for y in 0..16 {
            assert_eq!(f.row_bits('A', y), FONT_8X8.row_bits('A', y / 2));
        }
        assert_eq!(f.row_bits('A', 16), 0);
    }

    /// 'A'（0x41）だけに絵があり、0x01 にも絵がある（コードページ 437 の顔）VGA の並びのフォント
    static VGA_TEST: [u8; 256 * 16] = {
        let mut d = [0u8; 256 * 16];
        d[0x41 * 16 + 2] = 0b1000_0001;
        d[0x41 * 16 + 15] = 0b1100_0000;
        d[0x3F * 16 + 5] = 0b0001_1000;
        d[0x01 * 16 + 3] = 0xFF;
        d
    };

    #[test]
    fn vga16_reverses_bits_to_left_first() {
        let f = Font::vga16(&VGA_TEST);
        assert_eq!((f.width, f.height), (8, 16));
        assert_eq!(f.row_bits('A', 2), 0b1000_0001);
        // bit 7（左端）が bit 0 になる
        assert_eq!(f.row_bits('A', 15), 0b0000_0011);
        assert!(f.pixel('A', 0, 15) && f.pixel('A', 1, 15) && !f.pixel('A', 2, 15));
        assert_eq!(f.row_bits('A', 16), 0);
    }

    #[test]
    fn vga16_uses_only_printable_ascii() {
        let f = Font::vga16(&VGA_TEST);
        // 空白は絵が無くても字。制御文字は BIOS に絵があっても代替文字
        assert!(f.has_glyph(' ') && f.has_glyph('~') && !f.has_glyph('\u{1}') && !f.has_glyph('é'));
        assert_eq!(f.row_bits('\u{1}', 5), f.row_bits('?', 5));
        assert_eq!(f.row_bits('\u{1}', 3), 0);
    }

    #[test]
    fn unknown_chars_use_replacement() {
        assert!(!FONT_8X8.has_glyph('あ'));
        assert_eq!(render('あ'), render('?'));
        assert_eq!(render('\u{1}'), render('?'));
    }
}
