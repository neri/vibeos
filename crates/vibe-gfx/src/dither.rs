//! 画像を白黒にする（明るさの Floyd–Steinberg の誤差拡散。docs/plans/mono.md §3.7）
//!
//! 画面は白（15）と黒（0）だけなので、写真や色の付いた画像は明るさだけを見て、誤差拡散で階調を表す。
//! 結果は色番号の並び（0 か 15）。画面に出すときの白黒の模様（`mono`）は 0 と 15 をそのまま出す。

use alloc::vec;
use alloc::vec::Vec;

use crate::palette::{BLACK, WHITE};

/// 8bit RGB の明るさ（Rec. 601。0〜255）
pub fn luma(c: [u8; 3]) -> i32 {
    (299 * c[0] as i32 + 587 * c[1] as i32 + 114 * c[2] as i32) / 1000
}

/// w × h の画像（`pixel(x, y)` が 8bit RGB）を誤差拡散で白黒の色番号の並び（左上から。0 か 15）にする
pub fn to_mono(w: usize, h: usize, mut pixel: impl FnMut(usize, usize) -> [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    // 今の行と次の行に持ち越す誤差（明るさ, 8bit の単位 × 16）
    let mut err = vec![0i32; w + 2];
    let mut next = vec![0i32; w + 2];
    for y in 0..h {
        for x in 0..w {
            let want = (luma(pixel(x, y)) + err[x + 1] / 16).clamp(0, 255);
            let (c, got) = if want >= 128 { (WHITE, 255) } else { (BLACK, 0) };
            out.push(c);
            let e = want - got;
            err[x + 2] += e * 7;
            next[x] += e * 3;
            next[x + 1] += e * 5;
            next[x + 2] += e;
        }
        core::mem::swap(&mut err, &mut next);
        next.iter_mut().for_each(|e| *e = 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_stay_solid() {
        assert!(to_mono(8, 4, |_, _| [0, 0, 0]).iter().all(|&c| c == BLACK));
        assert!(to_mono(8, 4, |_, _| [255, 255, 255]).iter().all(|&c| c == WHITE));
        // 明るい黄はほとんど白、紺（明るさ 6%）はほとんど黒
        let yellow = to_mono(16, 16, |_, _| [255, 255, 0]).iter().filter(|&&c| c == WHITE).count();
        assert!(yellow >= 16 * 16 * 85 / 100, "{yellow}");
        let navy = to_mono(16, 16, |_, _| [0, 0, 128]).iter().filter(|&&c| c == BLACK).count();
        assert!(navy >= 16 * 16 * 90 / 100, "{navy}");
    }

    #[test]
    fn gray_mixes_black_and_white_and_keeps_the_average() {
        let (w, h) = (32, 32);
        for v in [64u8, 128, 192] {
            let out = to_mono(w, h, |_, _| [v, v, v]);
            assert!(out.iter().all(|&c| c == BLACK || c == WHITE));
            let white = out.iter().filter(|&&c| c == WHITE).count() as u32;
            let avg = white * 255 / (w * h) as u32;
            assert!(avg.abs_diff(v as u32) <= 8, "{v}: {avg}");
        }
    }

    #[test]
    fn small_gradient_matches_snapshot() {
        // 黒から白への横のグラデーション（8x2）。結果が変わったら誤差拡散の仕方が変わった
        let out = to_mono(8, 2, |x, _| {
            let v = (x * 255 / 7) as u8;
            [v, v, v]
        });
        assert_eq!(out, SNAPSHOT);
    }

    const SNAPSHOT: [u8; 16] = [0, 0, 0, 15, 0, 15, 15, 15, 0, 0, 0, 15, 0, 15, 15, 15];
}
