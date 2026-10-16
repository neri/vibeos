//! VGA のモード 12h の 16 色（D-072）
//!
//! Windows の 16 色を CGA の並び（端末の ANSI の色番号と同じ）に置いたもの。値は VGA の DAC と同じ 6bit（0〜63）。

pub const BLACK: u8 = 0;
pub const BLUE: u8 = 1;
pub const GREEN: u8 = 2;
pub const CYAN: u8 = 3;
pub const RED: u8 = 4;
pub const MAGENTA: u8 = 5;
pub const BROWN: u8 = 6;
pub const LIGHT_GRAY: u8 = 7;
pub const DARK_GRAY: u8 = 8;
pub const LIGHT_BLUE: u8 = 9;
pub const LIGHT_GREEN: u8 = 10;
pub const LIGHT_CYAN: u8 = 11;
pub const LIGHT_RED: u8 = 12;
pub const LIGHT_MAGENTA: u8 = 13;
pub const YELLOW: u8 = 14;
pub const WHITE: u8 = 15;

/// VGA のモード 12h の 16 色（各 6bit。DECISIONS.md D-072）。Windows の 16 色を CGA の並び（端末の ANSI の
/// 色番号と同じ。0〜15 の名前の定数がそのまま使える）に置いたもの。立体の縁に使う 4 段の灰色
/// （黒・128・192・白）を持つ
pub const VGA16: [[u8; 3]; 16] = [
    [0, 0, 0],    // 黒
    [0, 0, 32],   // 紺
    [0, 32, 0],   // 緑
    [0, 32, 32],  // 青緑
    [32, 0, 0],   // えんじ
    [32, 0, 32],  // 紫
    [32, 32, 0],  // オリーブ
    [48, 48, 48], // 明るい灰（192）
    [32, 32, 32], // 濃い灰（128）
    [0, 0, 63],   // 青
    [0, 63, 0],   // 明るい緑
    [0, 63, 63],  // 水色
    [63, 0, 0],   // 赤
    [63, 0, 63],  // 明るい紫
    [63, 63, 0],  // 黄
    [63, 63, 63], // 白
];

/// 色（各 6bit）に一番近い 16 色（`VGA16`）の番号
pub fn nearest16(c: [u8; 3]) -> u8 {
    let mut best = (i32::MAX, 0u8);
    for (i, v) in VGA16.iter().enumerate() {
        let (dr, dg, db) = (v[0] as i32 - c[0] as i32, v[1] as i32 - c[1] as i32, v[2] as i32 - c[2] as i32);
        let d = 3 * dr * dr + 4 * dg * dg + 2 * db * db;
        if d < best.0 {
            best = (d, i as u8);
        }
    }
    best.1
}

/// 6bit の値を 8bit に広げる（QEMU が VGA の DAC の色を画面に出すときと同じ式）
pub fn expand6(v: u8) -> u8 {
    let v = v & 0x3F;
    let b = v & 1;
    (v << 2) | (b << 1) | b
}

/// パレットの色（各 6bit）を 32 ビットの色（0x00RRGGBB）に
pub fn rgb32(c: [u8; 3]) -> u32 {
    (expand6(c[0]) as u32) << 16 | (expand6(c[1]) as u32) << 8 | expand6(c[2]) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vga16_is_exact_in_32bit() {
        // QEMU が DAC の 6bit を広げたときに Windows の 16 色の値そのものになる
        let lut: [u32; 16] = core::array::from_fn(|i| rgb32(VGA16[i]));
        assert_eq!(lut[LIGHT_GRAY as usize], 0x00C0_C0C0);
        assert_eq!(lut[DARK_GRAY as usize], 0x0080_8080);
        assert_eq!(lut[BLUE as usize], 0x0000_0080);
        assert_eq!(lut[YELLOW as usize], 0x00FF_FF00);
        assert_eq!(lut[WHITE as usize], 0x00FF_FFFF);
    }

    #[test]
    fn nearest16_finds_each_color() {
        for (i, &c) in VGA16.iter().enumerate() {
            assert_eq!(nearest16(c) as usize, i);
        }
        assert_eq!(nearest16([60, 2, 3]), LIGHT_RED);
        assert_eq!(nearest16([44, 44, 44]), LIGHT_GRAY);
        assert_eq!(nearest16([28, 28, 28]), DARK_GRAY);
    }

    #[test]
    fn expand6_matches_qemu() {
        assert_eq!(expand6(0), 0);
        assert_eq!(expand6(63), 255);
        assert_eq!(expand6(32), 128);
        assert_eq!(expand6(48), 192);
    }
}
