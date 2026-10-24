//! ガウスぼかし（すりガラスの後ろをぼかす）
//!
//! 幅 `2k + 1` の箱（範囲の平均）を横・縦それぞれ 3 回かけて、ガウス関数のぼかしに近づける。3 回かけた重みは
//! 2 次のスプラインになり、標準偏差 σ² = 3 × ((2k + 1)² − 1) / 12 = k(k + 1) のガウス関数にほぼ一致する。
//! 箱は「合計に 1 つ足して 1 つ引く」だけなので、1 ピクセルの計算の量が k によらない。整数だけで計算する
//! （カーネルで使うため。割り算は表を引いて済ませる）。画像の外は端のピクセルを伸ばす（clamp）。
//! ぼかしが届く幅は片側 `3k`。

use alloc::boxed::Box;
use alloc::vec;

/// `k` の上限（RB を 16 ビットの欄 2 つで合計するので、255 × (2k + 1) が 16 ビットに収まるように）
pub const MAX_K: usize = 63;

/// RB と G を別々に、16 ビットの欄 2 つずつで合計する
const RB: u32 = 0x00FF_00FF;

/// 割り算の表の大きさ（合計は 255 × 127 = 32385 まで。2 の冪にして、引く前の範囲の確かめを省けるようにする）
const DIV_LEN: usize = 32768;

/// 幅 `2k + 1` の箱のぼかし。割り算の表（合計 → 丸めた平均）を持つ
pub struct Blur {
    k: usize,
    div: Box<[u8; DIV_LEN]>,
}

impl Blur {
    pub fn new(k: usize) -> Blur {
        assert!(k <= MAX_K, "blur radius too large");
        let n = 2 * k as u32 + 1;
        // 32KB なのでスタックを通さずにヒープに取る（カーネルのスタックは 16KB）
        let mut div: Box<[u8; DIV_LEN]> = vec![0u8; DIV_LEN].into_boxed_slice().try_into().unwrap();
        for (s, d) in div.iter_mut().enumerate().take(255 * n as usize + 1) {
            *d = ((s as u32 + n / 2) / n) as u8;
        }
        Blur { k, div }
    }

    /// ぼかしが届く幅（片側）
    pub fn reach(&self) -> usize {
        3 * self.k
    }

    /// 箱の平均を 1 回: `dst[lo..hi]` に、`src` の前後 `k` 個（並びの外は端の値）の平均を書く。
    /// `src` のうち読むのは `[lo − k, hi + k)` を並びの中に切り詰めた所だけ
    fn pass(&self, src: &[u32], dst: &mut [u32], lo: usize, hi: usize) {
        let (k, last) = (self.k, src.len() - 1);
        assert!(hi <= src.len() && hi <= dst.len());
        let mask = DIV_LEN - 1;
        let at = |i: isize| src[i.clamp(0, last as isize) as usize];
        let (mut rb, mut g) = (0u32, 0u32);
        for j in lo as isize - k as isize..=(lo + k) as isize {
            let c = at(j);
            rb += c & RB;
            g += (c >> 8) & 0xFF;
        }
        let div = &*self.div;
        for i in lo..hi {
            let (r, gg, b) = ((rb >> 16) as usize & mask, g as usize & mask, (rb & 0xFFFF) as usize & mask);
            dst[i] = (div[r] as u32) << 16 | (div[gg] as u32) << 8 | div[b] as u32;
            // 箱を 1 つ右へ: i + k + 1 を足して i − k を引く（先に足すので欄が負にならない）
            let (add, sub) = (src[(i + k + 1).min(last)], src[i.saturating_sub(k)]);
            rb = rb + (add & RB) - (sub & RB);
            g = g + ((add >> 8) & 0xFF) - ((sub >> 8) & 0xFF);
        }
    }

    /// 長さ `n` の並び `a` を 3 回ぼかし、`[lo, hi)` を正しく `b` に書く（その外は途中の値）。
    /// 回ごとに、次の回が読む範囲だけを計算する
    fn line(&self, a: &mut [u32], b: &mut [u32], lo: usize, hi: usize) {
        let (k, n) = (self.k, a.len());
        let (l1, h1) = (lo.saturating_sub(2 * k), (hi + 2 * k).min(n));
        let (l2, h2) = (lo.saturating_sub(k), (hi + k).min(n));
        self.pass(a, b, l1, h1);
        self.pass(b, a, l2, h2);
        self.pass(a, b, lo, hi);
    }

    /// `w` × `h` の 0x00RRGGBB の画像 `img` をぼかす（縦・横に 3 回ずつ）
    pub fn apply(&self, img: &mut [u32], w: usize, h: usize, tmp: &mut [u32]) {
        self.apply_region(img, w, h, tmp, (0, 0, w, h));
    }

    /// `img` をぼかし、そのうち矩形 `[x0, x1) × [y0, y1)` だけを正しく書く（外は途中の値が残る）。
    /// 画像の外は端の値で、画像全体をぼかしたときと同じ結果になる。縦 → 横の順にかけ、縦は必要な行だけ、
    /// 横は必要な行の、必要な列だけを計算する（タイトルバーは上下にぼかしの届く幅を足した画像の真ん中の行だけ要る）。
    /// `tmp` は作業用で、`2 * max(w, h)` 個以上要る（カーネルではスタックに置かないよう、呼ぶ側が用意する）
    pub fn apply_region(&self, img: &mut [u32], w: usize, h: usize, tmp: &mut [u32], region: (usize, usize, usize, usize)) {
        let (x0, y0, x1, y1) = region;
        assert!(img.len() >= w * h && x0 <= x1 && x1 <= w && y0 <= y1 && y1 <= h);
        let n = w.max(h);
        assert!(tmp.len() >= 2 * n, "blur work buffer too small");
        if x0 == x1 || y0 == y1 || self.k == 0 {
            return;
        }
        let (a, b) = tmp.split_at_mut(n);
        // 縦: 横の回で読む列（[x0 − 3k, x1 + 3k)）の、必要な行だけ
        let (cx0, cx1) = (x0.saturating_sub(self.reach()), (x1 + self.reach()).min(w));
        for x in cx0..cx1 {
            for (d, &p) in a[..h].iter_mut().zip(img[x..].iter().step_by(w)) {
                *d = p;
            }
            self.line(&mut a[..h], &mut b[..h], y0, y1);
            for (p, &v) in img[y0 * w + x..].iter_mut().step_by(w).zip(&b[y0..y1]) {
                *p = v;
            }
        }
        // 横: 必要な行の、必要な列だけ（読む列は縦で計算したもの）
        for y in y0..y1 {
            let row = &mut img[y * w..(y + 1) * w];
            a[cx0..cx1].copy_from_slice(&row[cx0..cx1]);
            self.line(&mut a[cx0..cx1], &mut b[cx0..cx1], x0 - cx0, x1 - cx0);
            row[x0..x1].copy_from_slice(&b[x0..x1]);
        }
    }
}

/// `w` × `h` の画像 `img` を、横・縦に 3 回ずつ幅 `2k + 1` の箱でぼかす（`Blur::apply`。表をその場で作る）
pub fn blur_rgb(img: &mut [u32], w: usize, h: usize, k: usize, tmp: &mut [u32]) {
    Blur::new(k).apply(img, w, h, tmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::rgb;

    fn blur(img: &[u32], w: usize, h: usize, k: usize) -> Vec<u32> {
        let mut out = img.to_vec();
        let mut tmp = vec![0; 2 * w.max(h)];
        blur_rgb(&mut out, w, h, k, &mut tmp);
        out
    }

    /// 本物のガウスぼかし（f64。σ² = k(k + 1)、±4σ で打ち切り、端は clamp、横 → 縦）
    fn gaussian(img: &[u32], w: usize, h: usize, k: usize) -> Vec<u32> {
        let sigma = ((k * (k + 1)) as f64).sqrt();
        let r = (4.0 * sigma).ceil() as isize;
        let weights: Vec<f64> = (-r..=r).map(|d| (-((d * d) as f64) / (2.0 * sigma * sigma)).exp()).collect();
        let total: f64 = weights.iter().sum();
        let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f64;
        // 成分ごとの f64 の画像
        let mut planes: Vec<Vec<f64>> = [16, 8, 0].iter().map(|&s| img.iter().map(|&c| ch(c, s)).collect()).collect();
        for p in planes.iter_mut() {
            let src = p.clone();
            for y in 0..h {
                for x in 0..w {
                    let v: f64 = (-r..=r)
                        .map(|d| src[y * w + (x as isize + d).clamp(0, w as isize - 1) as usize] * weights[(d + r) as usize])
                        .sum();
                    p[y * w + x] = v / total;
                }
            }
            let src = p.clone();
            for y in 0..h {
                for x in 0..w {
                    let v: f64 = (-r..=r)
                        .map(|d| src[(y as isize + d).clamp(0, h as isize - 1) as usize * w + x] * weights[(d + r) as usize])
                        .sum();
                    p[y * w + x] = v / total;
                }
            }
        }
        (0..w * h)
            .map(|i| {
                let c = |p: &Vec<f64>| p[i].round().clamp(0.0, 255.0) as u32;
                c(&planes[0]) << 16 | c(&planes[1]) << 8 | c(&planes[2])
            })
            .collect()
    }

    fn max_diff(a: &[u32], b: &[u32]) -> u32 {
        a.iter()
            .zip(b)
            .map(|(&p, &q)| [16, 8, 0].iter().map(|&s| ((p >> s) & 0xFF).abs_diff((q >> s) & 0xFF)).max().unwrap())
            .max()
            .unwrap()
    }

    /// 決まった種の乱数（xorshift）
    fn noise(n: usize, mut seed: u32) -> Vec<u32> {
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed & 0x00FF_FFFF
            })
            .collect()
    }

    #[test]
    fn uniform_image_is_unchanged() {
        for c in [rgb(0, 0, 0), rgb(255, 255, 255), rgb(83, 74, 183), rgb(1, 254, 128)] {
            for k in [1, 6, 9, MAX_K] {
                let img = vec![c; 40 * 30];
                assert_eq!(blur(&img, 40, 30, k), img, "color {c:06x} k={k}");
            }
        }
    }

    #[test]
    fn square_spreads_symmetrically_and_keeps_brightness() {
        let (w, h, k) = (61usize, 61usize, 6usize);
        let mut img = vec![0u32; w * h];
        // 1 点だけでは丸めで消えるので、真ん中に 21x21 の明るい四角を置く
        for y in 20..41 {
            for x in 20..41 {
                img[y * w + x] = rgb(200, 100, 50);
            }
        }
        let out = blur(&img, w, h, k);
        let at = |x: usize, y: usize| out[y * w + x];
        for y in 0..h {
            for x in 0..w {
                assert_eq!(at(x, y), at(w - 1 - x, y), "({x},{y})");
                assert_eq!(at(x, y), at(x, h - 1 - y), "({x},{y})");
            }
        }
        // 真ん中から外へ向かって暗くなる
        let line: Vec<u32> = (30..w).map(|x| at(x, 30) >> 16).collect();
        assert!(line.windows(2).all(|p| p[0] >= p[1]), "{line:?}");
        // 明るさの合計がほぼ保たれる（四角の外へ逃げた分も画像の中にある）
        let sum = |img: &[u32], s: u32| img.iter().map(|&c| ((c >> s) & 0xFF) as i64).sum::<i64>();
        for s in [16, 8, 0] {
            let (before, after) = (sum(&img, s), sum(&out, s));
            assert!((before - after).abs() * 100 <= before, "channel {s}: {before} -> {after}");
        }
    }

    #[test]
    fn close_to_true_gaussian() {
        for k in [3, 6, 9] {
            // 段差（左右と上下）
            let (w, h) = (80usize, 50usize);
            let step: Vec<u32> = (0..w * h)
                .map(|i| if i % w < 37 { rgb(255, 255, 255) } else if i / w < 20 { rgb(0, 0, 0) } else { rgb(83, 74, 183) })
                .collect();
            let d = max_diff(&blur(&step, w, h, k), &gaussian(&step, w, h, k));
            assert!(d <= 4, "step k={k}: {d}");
            // 乱数（細かい模様）は重みの形の違いがいちばん出る。1 点の差は 20 前後になるが、平均は 1 前後
            let rnd = noise(w * h, 12345 + k as u32);
            let (a, b) = (blur(&rnd, w, h, k), gaussian(&rnd, w, h, k));
            let total: u32 = a
                .iter()
                .zip(&b)
                .map(|(&p, &q)| [16, 8, 0].iter().map(|&s| ((p >> s) & 0xFF).abs_diff((q >> s) & 0xFF)).sum::<u32>())
                .sum();
            let mean = total as f64 / (3 * w * h) as f64;
            assert!(mean <= 1.5, "noise k={k}: mean {mean}");
            assert!(max_diff(&a, &b) <= 32, "noise k={k}");
        }
    }

    #[test]
    fn region_matches_whole_image() {
        let (w, h) = (90usize, 64usize);
        let img = noise(w * h, 99);
        let full = blur(&img, w, h, 6);
        let b = Blur::new(6);
        let mut tmp = vec![0; 2 * w];
        // 真ん中・端に付いたもの・ぼかしの幅より細いもの
        for region in [(18, 18, 72, 42), (0, 0, 30, 10), (60, 50, 90, 64), (40, 0, 41, 64), (0, 30, 90, 31)] {
            let mut out = img.clone();
            b.apply_region(&mut out, w, h, &mut tmp, region);
            let (x0, y0, x1, y1) = region;
            for y in y0..y1 {
                assert_eq!(out[y * w + x0..y * w + x1], full[y * w + x0..y * w + x1], "{region:?} row {y}");
            }
        }
    }

    #[test]
    fn tiny_images_do_not_panic() {
        for (w, h) in [(1, 1), (1, 7), (7, 1), (3, 2), (12, 12)] {
            let img = noise(w * h, 7);
            let out = blur(&img, w, h, 6);
            assert_eq!(out.len(), w * h);
        }
        // 幅 0
        let mut empty: [u32; 0] = [];
        blur_rgb(&mut empty, 0, 5, 6, &mut [0; 10]);
    }

    #[test]
    fn outside_is_clamped_to_the_edge() {
        // 左の 30 列は赤、右は青。端から 3k より内側でも、外は端の色と同じに数えるので左端は赤のまま
        let (w, h, k) = (60usize, 10usize, 6usize);
        let img: Vec<u32> = (0..w * h).map(|i| if i % w < 30 { rgb(255, 0, 0) } else { rgb(0, 0, 255) }).collect();
        let out = blur(&img, w, h, k);
        for y in 0..h {
            assert_eq!(out[y * w], rgb(255, 0, 0));
            assert_eq!(out[y * w + w - 1], rgb(0, 0, 255));
            // 上下の端も（縦は一様なので変わらない）
            assert_eq!(out[y * w + 10], out[10]);
        }
        // 段差の所は混ざる
        assert!(out[29] >> 16 < 255 && out[29] & 0xFF > 0);
    }
}
