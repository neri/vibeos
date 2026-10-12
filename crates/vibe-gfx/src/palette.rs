//! 標準パレット（DESIGN.md §8.3）
//!
//! | 番号    | 内容                         |
//! |---------|------------------------------|
//! | 0–15    | CGA 互換 16 色               |
//! | 16–31   | グレースケール 16 段階       |
//! | 32–247  | 6×6×6 カラーキューブ         |
//! | 248–255 | システム予約（254/255 はマウスカーソルの黒/白） |
//!
//! 値は VGA DAC と同じ 6bit（0〜63）。

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

pub const GRAY_BASE: u8 = 16;
pub const CUBE_BASE: u8 = 32;

/// デスクトップの背景の色（落ち着いた青緑。カラーキューブの 0,2,1）
pub const DESKTOP_BG: u8 = CUBE_BASE + 13;
pub const RESERVED_BASE: u8 = 248;
/// マウスカーソル用（予約領域）
pub const CURSOR_BLACK: u8 = 254;
pub const CURSOR_WHITE: u8 = 255;

const CGA: [[u8; 3]; 16] = [
    [0, 0, 0],
    [0, 0, 42],
    [0, 42, 0],
    [0, 42, 42],
    [42, 0, 0],
    [42, 0, 42],
    [42, 21, 0],
    [42, 42, 42],
    [21, 21, 21],
    [21, 21, 63],
    [21, 63, 21],
    [21, 63, 63],
    [63, 21, 21],
    [63, 21, 63],
    [63, 63, 21],
    [63, 63, 63],
];

/// カラーキューブの 1 軸の段階（6bit）
const CUBE_LEVELS: [u8; 6] = [0, 13, 25, 38, 50, 63];

/// 標準パレット 256 色（各 6bit）
pub fn standard() -> [[u8; 3]; 256] {
    let mut p = [[0u8; 3]; 256];
    p[..16].copy_from_slice(&CGA);
    for i in 0..16 {
        let v = (i * 63 / 15) as u8;
        p[GRAY_BASE as usize + i] = [v, v, v];
    }
    for r in 0..6 {
        for g in 0..6 {
            for b in 0..6 {
                p[cube_index(r, g, b) as usize] = [CUBE_LEVELS[r], CUBE_LEVELS[g], CUBE_LEVELS[b]];
            }
        }
    }
    // 予約領域は当面白にしておく（カーソル用の黒を除く）
    for c in p.iter_mut().skip(RESERVED_BASE as usize) {
        *c = [63, 63, 63];
    }
    p[CURSOR_BLACK as usize] = [0, 0, 0];
    p
}

fn cube_index(r: usize, g: usize, b: usize) -> u8 {
    CUBE_BASE + (r * 36 + g * 6 + b) as u8
}

/// 8bit RGB に近いカラーキューブの色番号
pub fn rgb(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| ((v as usize * 5 + 127) / 255).min(5);
    cube_index(level(r), level(g), level(b))
}

/// パレット `pal`（各 6bit）の中で 8bit RGB に一番近い色の番号（予約領域は除く）
pub fn nearest(pal: &[[u8; 3]; 256], r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = ((r >> 2) as i32, (g >> 2) as i32, (b >> 2) as i32);
    let mut best = (i32::MAX, 0u8);
    for (i, c) in pal.iter().enumerate().take(RESERVED_BASE as usize) {
        let (dr, dg, db) = (c[0] as i32 - r, c[1] as i32 - g, c[2] as i32 - b);
        // 人の目は緑に敏感なので重みを付ける
        let d = 3 * dr * dr + 4 * dg * dg + 2 * db * db;
        if d < best.0 {
            best = (d, i as u8);
            if d == 0 {
                break;
            }
        }
    }
    best.1
}

/// 6bit の値を 8bit に広げる（QEMU が VGA の DAC の色を画面に出すときと同じ式。32 ビットの画面で
/// パレットの画像を出したときに、8 ビットの画面と同じ色になるように）
pub fn expand6(v: u8) -> u8 {
    let v = v & 0x3F;
    let b = v & 1;
    (v << 2) | (b << 1) | b
}

/// パレットの色（各 6bit）を 32 ビットの色（0x00RRGGBB）に
pub fn rgb32(c: [u8; 3]) -> u32 {
    (expand6(c[0]) as u32) << 16 | (expand6(c[1]) as u32) << 8 | expand6(c[2]) as u32
}

/// パレット全体を「色番号 → 32 ビットの色」の表に
pub fn lut32(pal: &[[u8; 3]; 256]) -> [u32; 256] {
    core::array::from_fn(|i| rgb32(pal[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let p = standard();
        assert_eq!(p[BLACK as usize], [0, 0, 0]);
        assert_eq!(p[WHITE as usize], [63, 63, 63]);
        assert_eq!(p[GRAY_BASE as usize + 15], [63, 63, 63]);
        assert_eq!(p[CUBE_BASE as usize], [0, 0, 0]);
        assert_eq!(p[CUBE_BASE as usize + 215], [63, 63, 63]);
        assert!(p.iter().flatten().all(|&v| v <= 63));
    }

    #[test]
    fn nearest_finds_exact_and_close_colors() {
        let p = standard();
        assert_eq!(nearest(&p, 0, 0, 0), BLACK);
        assert_eq!(nearest(&p, 255, 255, 255), WHITE);
        assert_eq!(p[nearest(&p, 250, 5, 5) as usize], [63, 0, 0]);
        let gray = nearest(&p, 128, 128, 128);
        let c = p[gray as usize];
        assert!(c[0] == c[1] && c[1] == c[2] && (28..=36).contains(&c[0]), "{c:?}");
    }

    #[test]
    fn expand6_matches_qemu() {
        assert_eq!(expand6(0), 0);
        assert_eq!(expand6(63), 255);
        assert_eq!(expand6(42), 168);
        assert_eq!(expand6(25), 103);
        assert_eq!(expand6(13), 55);
        assert_eq!(rgb32([63, 63, 21]), 0x00FF_FF57);
        assert_eq!(lut32(&standard())[BLUE as usize], 0x0000_00A8);
    }

    #[test]
    fn rgb_maps_to_cube() {
        let p = standard();
        assert_eq!(p[rgb(255, 0, 0) as usize], [63, 0, 0]);
        assert_eq!(p[rgb(0, 128, 255) as usize], [0, 38, 63]);
        assert_eq!(rgb(0, 0, 0), CUBE_BASE);
    }
}
