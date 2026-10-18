//! 色番号（VGA の 16 色）から白黒への置き換え（docs/plans/mono.md §3.1、D-077）
//!
//! 画面は白（15）と黒（0）の画素だけで出す。アプリやウィンドウのバッファは今までどおり 16 色の色番号で、
//! 画面に送る直前に、色番号を明るさの 5 段の模様（4x4 の順序付きディザ）にする。0 と 15 はベタ、
//! 濃い灰（8）は 50% の市松。模様は画面の座標にそろえる（ウィンドウを動かしても模様の向きは変わらない）。
//! 1 バイトに 8 ピクセル（最上位ビットが左端）、ビットが 1 なら白。ハードウェアには触らない
//! （書き込みは kernel/src/drivers/vga.rs）。

/// 色番号ごとの段（白の割合 0・1/4・1/2・3/4・1）。明るさ（Rec. 601 の輝度）で分けた
pub const LEVEL: [u8; 16] = [
    0, // 黒
    0, // 紺
    1, // 緑
    1, // 青緑
    0, // えんじ
    1, // 紫
    2, // オリーブ
    3, // 明るい灰
    2, // 濃い灰
    0, // 青
    2, // 明るい緑
    3, // 水色
    1, // 赤
    2, // 明るい紫
    4, // 黄
    4, // 白
];

/// 4x4 の順序付きディザの閾値（0〜15）。段 l の画素は閾値が 4l 未満なら白
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// 画面の (x, y) にある色番号 `c` の画素が白か
pub fn is_white(c: u8, x: usize, y: usize) -> bool {
    BAYER[y & 3][x & 3] < LEVEL[(c & 15) as usize] * 4
}

/// `y` 行目の、色番号ごとの 8 ピクセル分の模様（x が 8 の倍数から始まる 1 バイト）。
/// 模様の周期は横 4 なので、1 行の中ではどのバイトでも同じ
pub fn row_masks(y: usize) -> [u8; 16] {
    core::array::from_fn(|c| {
        let mut m = 0u8;
        for x in 0..8 {
            if is_white(c as u8, x, y) {
                m |= 0x80 >> x;
            }
        }
        m
    })
}

/// 8 ピクセル（足りなければ残りは黒）を 1 バイトにする。色番号の上位 4 ビットは見ない
pub fn pack8(px: &[u8], masks: &[u8; 16]) -> u8 {
    let mut v = 0;
    for (i, &c) in px.iter().take(8).enumerate() {
        v |= masks[(c & 15) as usize] & (0x80 >> i);
    }
    v
}

/// `y` 行目の色番号の 1 行（左端が x = 0。8 の倍数から始めること）を 8 ピクセルずつ `out` に詰める。
/// 書いた数（`row` の 8 ピクセル単位の切り上げと `out` の長さの小さい方）を返す
pub fn pack_row(row: &[u8], y: usize, out: &mut [u8]) -> usize {
    let masks = row_masks(y);
    let mut n = 0;
    for (o, px) in out.iter_mut().zip(row.chunks(8)) {
        *o = pack8(px, &masks);
        n += 1;
    }
    n
}

/// 白黒の 1 ビットの絵の色を入れ替える（黒 ↔ 白。ほかの色番号と透明はそのまま）。
/// 黒い地に白黒のアイコンを出すときに使う
pub fn invert(c: u8) -> u8 {
    match c {
        0 => 15,
        15 => 0,
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{BLACK, DARK_GRAY, LIGHT_GRAY, VGA16, WHITE};

    #[test]
    fn black_and_white_are_solid() {
        for y in 0..8 {
            let m = row_masks(y);
            assert_eq!(m[BLACK as usize], 0x00);
            assert_eq!(m[WHITE as usize], 0xFF);
            assert_eq!(m[14], 0xFF, "yellow is white");
            assert_eq!(m[1], 0x00, "navy is black");
        }
    }

    #[test]
    fn dark_gray_is_a_checkerboard() {
        // 行ごとに 1 つずつずれる市松
        assert_eq!(row_masks(0)[DARK_GRAY as usize], 0xAA);
        assert_eq!(row_masks(1)[DARK_GRAY as usize], 0x55);
        assert_eq!(row_masks(2)[DARK_GRAY as usize], 0xAA);
        assert_eq!(row_masks(3)[DARK_GRAY as usize], 0x55);
    }

    #[test]
    fn each_level_has_its_share_of_white() {
        for c in 0..16u8 {
            let white = (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).filter(|&(x, y)| is_white(c, x, y)).count();
            assert_eq!(white, LEVEL[c as usize] as usize * 4, "color {c}");
        }
        // 明るい灰は 4 つに 1 つが黒（4 行 × 8 ピクセルのうち 24 が白）
        let white: u32 = (0..4).map(|y| row_masks(y)[LIGHT_GRAY as usize].count_ones()).sum();
        assert_eq!(white, 24);
    }

    #[test]
    fn levels_follow_brightness() {
        // 明るい色ほど段が低くならない
        let luma = |c: [u8; 3]| 299 * c[0] as u32 + 587 * c[1] as u32 + 114 * c[2] as u32;
        let mut order: Vec<usize> = (0..16).collect();
        order.sort_by_key(|&i| luma(VGA16[i]));
        for w in order.windows(2) {
            assert!(LEVEL[w[0]] <= LEVEL[w[1]], "{} brighter than {} but darker level", w[1], w[0]);
        }
    }

    #[test]
    fn pattern_repeats_every_four_pixels() {
        for y in 0..4 {
            for c in 0..16u8 {
                for x in 0..4 {
                    assert_eq!(is_white(c, x, y), is_white(c, x + 4, y + 4));
                    assert_eq!(is_white(c, x, y), is_white(c, x + 8, y));
                }
            }
        }
    }

    #[test]
    fn pack8_places_leftmost_pixel_in_top_bit() {
        let m = row_masks(0);
        assert_eq!(pack8(&[15, 0, 0, 0, 0, 0, 0, 0], &m), 0x80);
        assert_eq!(pack8(&[0, 0, 0, 0, 0, 0, 0, 15], &m), 0x01);
        assert_eq!(pack8(&[0x1F; 8], &m), 0xFF, "upper bits are ignored");
    }

    #[test]
    fn short_tail_is_padded_with_black() {
        let row = [WHITE; 11];
        let mut out = [0x33u8; 4];
        assert_eq!(pack_row(&row, 0, &mut out), 2);
        assert_eq!(out[0], 0xFF);
        assert_eq!(out[1], 0xE0);
        assert_eq!(out[2], 0x33);
    }

    #[test]
    fn pack_row_stops_at_output_length() {
        let row = [DARK_GRAY; 640];
        let mut out = [0u8; 10];
        assert_eq!(pack_row(&row, 1, &mut out), 10);
        assert!(out.iter().all(|&b| b == 0x55));
    }

    #[test]
    fn invert_swaps_only_black_and_white() {
        assert_eq!(invert(BLACK), WHITE);
        assert_eq!(invert(WHITE), BLACK);
        assert_eq!(invert(DARK_GRAY), DARK_GRAY);
        assert_eq!(invert(0xFF), 0xFF);
    }
}
