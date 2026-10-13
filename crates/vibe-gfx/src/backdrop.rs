//! 既定のデスクトップの背景: 地の色 1 色に、画面の大きさに対する割合で置いた大きな円を重ねたもの
//!
//! 画面の大きさの画素は作らず、1 行ずつ「その行で円が占める x の範囲」を整数の平方根で求めて塗る
//! （重ね合わせが 1 行ずつなので）。縁は 1 行を 4 本の細い行に分け、それぞれの範囲が各ピクセルを
//! 覆う長さ（1/16 ピクセル単位）を足した割合で下の色と混ぜて、なめらかにする。

use crate::canvas::{Color, blend, isqrt};


/// 円: 中心と半径は画面に対する 1/1000 単位（`cx` は幅、`cy` と `r` は高さに対して）
#[derive(Clone, Copy, Debug)]
pub struct Circle {
    pub cx: i32,
    pub cy: i32,
    pub r: i32,
    pub color: Color,
}

/// 1 ピクセルを縦に分ける数と、横の 1 ピクセルの細かさ
const SUB_ROWS: i64 = 4;
const SUB: i64 = 16;

/// 幅 `row.len()`・高さ `height` の画面の `y` 行目に、地 `base` と円 `circles`（後ろのものほど手前）を描く
pub fn fill_row(row: &mut [u32], y: usize, height: usize, base: Color, circles: &[Circle]) {
    row.fill(base);
    let width = row.len() as i64;
    for c in circles {
        // 1/16 ピクセル単位の中心と半径
        let cx = width * SUB * c.cx as i64 / 1000;
        let cy = height as i64 * SUB * c.cy as i64 / 1000;
        let r = height as i64 * SUB * c.r as i64 / 1000;
        // 細い行ごとの範囲 [l, r)（1/16 ピクセル）
        let mut spans = [None; SUB_ROWS as usize];
        for (k, s) in spans.iter_mut().enumerate() {
            let sy = y as i64 * SUB + (2 * k as i64 + 1) * SUB / (2 * SUB_ROWS);
            let dy = sy - cy;
            if dy.abs() < r {
                let n = (r * r - dy * dy) as u64;
                let half = match u32::try_from(n) {
                    Ok(n) => isqrt32(n) as i64,
                    Err(_) => isqrt(n) as i64,
                };
                *s = Some((cx - half, cx + half));
            }
        }
        let present = spans.iter().flatten();
        let (Some(lo), Some(hi)) = (present.clone().map(|s| s.0).min(), present.clone().map(|s| s.1).max()) else {
            continue;
        };
        // どの細い行でも覆われる所（中）と、縁の所
        let full = if spans.iter().all(|s| s.is_some()) {
            let l = spans.iter().flatten().map(|s| s.0).max().unwrap_or(0);
            let r = spans.iter().flatten().map(|s| s.1).min().unwrap_or(0);
            ((l + SUB - 1).div_euclid(SUB).clamp(0, width), r.div_euclid(SUB).clamp(0, width))
        } else {
            (0, 0)
        };
        let (x0, x1) = (lo.div_euclid(SUB).clamp(0, width), (hi + SUB - 1).div_euclid(SUB).clamp(0, width));
        if full.0 < full.1 {
            row[full.0 as usize..full.1 as usize].fill(c.color);
        }
        for x in x0..x1 {
            if x >= full.0 && x < full.1 {
                continue;
            }
            let (px0, px1) = (x * SUB, x * SUB + SUB);
            let cov: i64 = spans.iter().flatten().map(|&(l, r)| (r.min(px1) - l.max(px0)).max(0)).sum();
            let a = (cov * 255 / (SUB * SUB_ROWS)) as u8;
            if a > 0 {
                let p = &mut row[x as usize];
                *p = blend(*p, c.color, a);
            }
        }
    }
}

/// 32 ビットの整数の平方根（切り捨て）。1 桁ずつ求めるので割り算を使わない
/// （486 では 64 ビットの割り算が遅く、背景は 1 行ごとに何回も求めるため）
fn isqrt32(mut n: u32) -> u32 {
    let mut root = 0u32;
    let mut bit = 1u32 << 30;
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if n >= root + bit {
            n -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::rgb;
    use alloc::vec;
    use alloc::vec::Vec;

    const BASE: Color = rgb(0, 0, 0);
    const WHITE: Color = rgb(255, 255, 255);

    fn render(w: usize, h: usize, circles: &[Circle]) -> Vec<u32> {
        let mut img = vec![0; w * h];
        for (y, row) in img.chunks_mut(w).enumerate() {
            fill_row(row, y, h, BASE, circles);
        }
        img
    }

    #[test]
    fn circle_is_filled_inside_and_soft_at_the_edge() {
        // 100x100 の真ん中に半径 30
        let c = [Circle { cx: 500, cy: 500, r: 300, color: WHITE }];
        let img = render(100, 100, &c);
        let at = |x: usize, y: usize| img[y * 100 + x] & 0xFF;
        assert_eq!(at(50, 50), 255);
        assert_eq!(at(50, 21), 255);
        assert_eq!(at(5, 5), 0);
        assert_eq!(at(50, 85), 0);
        // 縁は途中の濃さが出る（なめらか）、上下左右で対称・斜めでほぼ対称
        let edge: Vec<u32> = (0..100).map(|x| at(x, 50)).filter(|&v| v != 0 && v != 255).collect();
        assert!(!edge.is_empty());
        for y in 0..100 {
            for x in 0..100 {
                assert_eq!(at(x, y), at(99 - x, y), "({x},{y})");
                assert_eq!(at(x, y), at(x, 99 - y), "({x},{y})");
                // 縦は 4 本の細い行で数えるので、斜めの対称は少しずれる
                assert!(at(x, y).abs_diff(at(y, x)) <= 48, "({x},{y})");
            }
        }
        // 塗った面積はおよそ π r²
        let area: u32 = img.iter().map(|p| p & 0xFF).sum::<u32>() / 255;
        assert!((2780..=2880).contains(&area), "{area}");
    }

    #[test]
    fn isqrt32_matches_isqrt() {
        for n in (0..100_000u32).chain([u32::MAX, u32::MAX - 1, 1 << 30, 65535 * 65535, 65535 * 65535 - 1]) {
            assert_eq!(isqrt32(n) as u64, isqrt(n as u64), "{n}");
        }
    }

    #[test]
    fn circles_scale_with_the_screen() {
        let c = [Circle { cx: 750, cy: 250, r: 200, color: WHITE }];
        for (w, h) in [(64, 40), (128, 80), (256, 160)] {
            let img = render(w, h, &c);
            // 中心は (3/4 w, 1/4 h)、半径 h/5
            assert_eq!(img[(h / 4) * w + 3 * w / 4], WHITE, "{w}x{h}");
            assert_eq!(img[(h / 4) * w + 3 * w / 4 - h / 5 - 2], BASE, "{w}x{h}");
            assert_eq!(img[(h / 4 + h / 5 + 2) * w + 3 * w / 4], BASE, "{w}x{h}");
        }
    }

    #[test]
    fn later_circles_are_on_top_and_off_screen_parts_are_clipped() {
        let c = [
            Circle { cx: 500, cy: 500, r: 400, color: rgb(10, 0, 0) },
            Circle { cx: 1000, cy: 0, r: 500, color: rgb(20, 0, 0) },
        ];
        let img = render(50, 50, &c);
        assert_eq!(img[2 * 50 + 47], rgb(20, 0, 0)); // 右上は 2 つ目
        assert_eq!(img[25 * 50 + 10], rgb(10, 0, 0));
        assert_eq!(img[49 * 50], BASE);
    }
}
