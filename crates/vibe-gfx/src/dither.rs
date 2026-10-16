//! 画像の色を VGA の 16 色（`palette::VGA16`）にする（近い色の表と Floyd–Steinberg の誤差拡散）
//!
//! モード 12h では画面の色は 16 色だけなので、写真や 256 色の画像は誤差拡散で階調を表す。
//! 16 色の表にある色はそのままの番号になる（誤差が出ない）。

use alloc::vec;
use alloc::vec::Vec;

use crate::palette::{VGA16, expand6, nearest16};

/// 8bit RGB の各成分の上位 4 ビット（4096 通り）→ 一番近い 16 色の番号。1 画素ごとに 16 色と
/// 比べると 486 では遅いので、表を 1 回作って引く。16 色の表の色（成分が 0・128・192・255）は
/// 上位 4 ビットの真ん中の値でもその色に一番近いので、そのままの番号になる
pub struct Lut16(Vec<u8>);

impl Lut16 {
    pub fn new() -> Lut16 {
        let mut t = vec![0u8; 4096];
        for (i, slot) in t.iter_mut().enumerate() {
            // 4 ビットの値 v の真ん中（v * 17）を 6 ビットにして比べる
            let c = |shift: usize| (((i >> shift) & 15) * 17 >> 2) as u8;
            *slot = nearest16([c(8), c(4), c(0)]);
        }
        Lut16(t)
    }

    /// 8bit RGB に近い 16 色の番号
    pub fn get(&self, r: u8, g: u8, b: u8) -> u8 {
        self.0[(r as usize >> 4) << 8 | (g as usize >> 4) << 4 | b as usize >> 4]
    }
}

impl Default for Lut16 {
    fn default() -> Self {
        Lut16::new()
    }
}

/// 16 色の番号 → 8bit RGB（QEMU と同じ広げ方）
pub fn vga16_rgb(i: u8) -> [u8; 3] {
    VGA16[(i & 15) as usize].map(expand6)
}

/// w × h の画像（`pixel(x, y)` が 8bit RGB）を誤差拡散で 16 色の番号の並び（左上から）にする
pub fn to_vga16(w: usize, h: usize, lut: &Lut16, mut pixel: impl FnMut(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    // 今の行と次の行に持ち越す誤差（RGB, 8bit の単位 × 16）
    let mut err = vec![[0i32; 3]; w + 2];
    let mut next = vec![[0i32; 3]; w + 2];
    for y in 0..h {
        for x in 0..w {
            let src = pixel(x, y);
            let want: [i32; 3] = core::array::from_fn(|i| (src[i] as i32 + err[x + 1][i] / 16).clamp(0, 255));
            let idx = lut.get(want[0] as u8, want[1] as u8, want[2] as u8);
            out.push(idx);
            let got = vga16_rgb(idx);
            for i in 0..3 {
                let e = want[i] - got[i] as i32;
                err[x + 2][i] += e * 7;
                next[x][i] += e * 3;
                next[x + 1][i] += e * 5;
                next[x + 2][i] += e;
            }
        }
        core::mem::swap(&mut err, &mut next);
        next.iter_mut().for_each(|e| *e = [0; 3]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};

    #[test]
    fn lut_returns_table_colors_exactly() {
        let lut = Lut16::new();
        for i in 0..16u8 {
            let [r, g, b] = vga16_rgb(i);
            assert_eq!(lut.get(r, g, b), i, "color {i}");
        }
        assert_eq!(lut.get(250, 250, 250), WHITE);
        assert_eq!(lut.get(10, 5, 0), BLACK);
    }

    #[test]
    fn table_colors_are_not_dithered() {
        let lut = Lut16::new();
        let out = to_vga16(5, 4, &lut, |x, _| vga16_rgb(x as u8 * 3));
        for row in out.chunks(5) {
            assert_eq!(row, [0, 3, 6, 9, 12]);
        }
    }

    #[test]
    fn mid_gray_mixes_neighbours_and_keeps_the_average() {
        let lut = Lut16::new();
        let (w, h) = (32, 32);
        let out = to_vga16(w, h, &lut, |_, _| [160, 160, 160]);
        // 128 と 192 の間の灰色は濃い灰と明るい灰だけで表し、平均は元の色に近い
        assert!(out.iter().all(|&c| c == DARK_GRAY || c == LIGHT_GRAY));
        let avg = out.iter().map(|&c| vga16_rgb(c)[0] as u32).sum::<u32>() / (w * h) as u32;
        assert!((156..=164).contains(&avg), "{avg}");
    }

    #[test]
    fn small_gradient_matches_snapshot() {
        // 黒から白への横のグラデーション（8x2）。結果が変わったら誤差拡散の仕方が変わった
        let lut = Lut16::new();
        let out = to_vga16(8, 2, &lut, |x, _| {
            let v = (x * 255 / 7) as u8;
            [v, v, v]
        });
        assert_eq!(out, SNAPSHOT);
    }

    const SNAPSHOT: [u8; 16] = [0, 0, 8, 8, 8, 7, 7, 15, 0, 0, 8, 8, 8, 7, 7, 15];
}
