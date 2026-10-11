//! 標準パレット（DESIGN.md §8.3）
//!
//! | 番号    | 内容                         |
//! |---------|------------------------------|
//! | 0–15    | CGA 互換 16 色               |
//! | 16–31   | グレースケール 16 段階       |
//! | 32–247  | 6×6×6 カラーキューブ         |
//! | 248–255 | システム予約                 |
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
pub const RESERVED_BASE: u8 = 248;

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
    // 予約領域は当面白にしておく
    for c in p.iter_mut().skip(RESERVED_BASE as usize) {
        *c = [63, 63, 63];
    }
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
    fn rgb_maps_to_cube() {
        let p = standard();
        assert_eq!(p[rgb(255, 0, 0) as usize], [63, 0, 0]);
        assert_eq!(p[rgb(0, 128, 255) as usize], [0, 38, 63]);
        assert_eq!(rgb(0, 0, 0), CUBE_BASE);
    }
}
