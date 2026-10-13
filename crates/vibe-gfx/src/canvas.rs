//! 32 ビットの画像（1 ピクセル = 0x00RRGGBB）への描画。
//!
//! 半透明の合成・角の丸い矩形・影・グラデーション・アルファ付きの画像を、整数の計算だけで描く
//! （カーネルでは FPU を使わない）。重い処理は「縁だけ」「先に計算した表」で済ませる:
//! 角の丸みは `Corner`（1/4 円の覆う割合の表）、影は `Shadow`（縁の濃さの 1 次元の表）を先に作って使い回す。
//! はみ出した部分は切り捨てる（座標は負でもよい）。

use alloc::vec;
use alloc::vec::Vec;

use vibe_font::{Font, VFont, vfn::SUBPIXELS};

use crate::palette;

/// 0x00RRGGBB
pub type Color = u32;

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

/// RB と G を別々に 2 つずつ並べて計算するためのマスク
const RB: u32 = 0x00FF_00FF;
const G: u32 = 0x0000_FF00;

/// 16 ビットごとの 2 つの値（それぞれ 0〜255*255）を 255 で割って丸める
#[inline]
fn div255_x2(x: u32) -> u32 {
    let x = x + 0x0080_0080;
    ((x + ((x >> 8) & RB)) >> 8) & RB
}

/// `dst` に `src` を濃さ `a`（0〜255）で重ねる
#[inline]
pub fn blend(dst: Color, src: Color, a: u8) -> Color {
    let (a, na) = (a as u32, 255 - a as u32);
    let rb = div255_x2((src & RB) * a + (dst & RB) * na);
    let g = div255_x2(((src & G) >> 8) * a + ((dst & G) >> 8) * na);
    rb | g << 8
}

/// 色の各成分に `a / 255` を掛ける（事前に掛けたアルファを作る）
#[inline]
fn scale(c: Color, a: u32) -> Color {
    div255_x2((c & RB) * a) | div255_x2(((c & G) >> 8) * a) << 8
}

/// 2 色の間の色（`t / n` の位置。t = 0 で `a`、t = n で `b`）
pub fn lerp(a: Color, b: Color, t: u32, n: u32) -> Color {
    if n == 0 {
        return a;
    }
    let ch = |c: Color, s: u32| (c >> s) & 0xFF;
    let mix = |s: u32| (ch(a, s) * (n - t) + ch(b, s) * t + n / 2) / n;
    mix(16) << 16 | mix(8) << 8 | mix(0)
}

/// グレーとみなす色の成分の差（テーマの少し温かい灰色と、それを白に混ぜたなめらかな文字の縁も含める）
const NEAR_GRAY: u8 = 12;

/// 32 ビットの色を標準のパレットの近い色の番号に（グレーに近い色はグレースケールの 16 段階、ほかはカラーキューブ）。
/// 少し色の付いた灰色をカラーキューブにすると、成分ごとの段階の境目で黄色や青に寄ってしまう
pub fn to_palette8(c: Color) -> u8 {
    let (r, g, b) = ((c >> 16) as u8, (c >> 8) as u8, c as u8);
    if r.max(g).max(b) - r.min(g).min(b) <= NEAR_GRAY {
        let v = (r as u32 + g as u32 + b as u32 + 1) / 3;
        return palette::GRAY_BASE + ((v * 15 + 127) / 255) as u8;
    }
    palette::rgb(r, g, b)
}

/// 角の丸み: 半径 `r` の 1/4 円が各ピクセルを覆う割合（0〜255）。左上の角の向きで、
/// `cov[y * r + x]` は角の外側の頂点から (x, y) のピクセル
pub struct Corner {
    pub r: usize,
    cov: Vec<u8>,
}

impl Corner {
    /// 1 ピクセルを 4×4 に分けて数える（17 段階）
    pub fn new(r: usize) -> Corner {
        let mut cov = vec![0u8; r * r];
        let c = (r * 8) as i64; // 円の中心（1/8 ピクセル単位）
        for y in 0..r {
            for x in 0..r {
                let mut n = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let px = (x * 8 + sx * 2 + 1) as i64 - c;
                        let py = (y * 8 + sy * 2 + 1) as i64 - c;
                        if px * px + py * py <= c * c {
                            n += 1;
                        }
                    }
                }
                cov[y * r + x] = (n * 255 / 16) as u8;
            }
        }
        Corner { r, cov }
    }

    /// 左上の角の向きで (x, y) の覆う割合
    pub fn coverage(&self, x: usize, y: usize) -> u8 {
        self.cov[y * self.r + x]
    }

    /// 矩形 (0, 0, w, h) の中の (x, y) の覆う割合。矩形の外と角の外は 0、角以外は 255。
    /// 矩形が小さくて半径が入らなければ、入る半径に縮める（表の内側の部分を使う）
    pub fn at(&self, x: i32, y: i32, w: i32, h: i32) -> u8 {
        if x < 0 || y < 0 || x >= w || y >= h {
            return 0;
        }
        let big = self.r as i32;
        let r = big.min(w / 2).min(h / 2);
        // 外側の辺からの距離（0 がいちばん外）
        let dx = if x < r { x } else if x >= w - r { w - 1 - x } else { return 255 };
        let dy = if y < r { y } else if y >= h - r { h - 1 - y } else { return 255 };
        let k = big - r;
        self.coverage((dx + k) as usize, (dy + k) as usize)
    }
}

/// 影: ぼかした矩形の縁の濃さ（0〜255）の 1 次元の表。矩形の辺を中心に、外へ `blur`・内へ `blur` の
/// 幅で 0 → 255 に変わる（なめらかな S 字）。ぼかした矩形は縦と横の表の積で表せる
pub struct Shadow {
    pub blur: usize,
    ramp: Vec<u8>,
}

impl Shadow {
    pub fn new(blur: usize) -> Shadow {
        let n = 2 * blur.max(1);
        let ramp = (0..n)
            .map(|i| {
                // t = (i + 0.5) / n を 1/4096 単位で。3t² − 2t³
                let t = ((2 * i + 1) * 4096 / (2 * n)) as u64;
                let s = (3 * t * t * 4096 - 2 * t * t * t) / (4096 * 4096);
                (s * 255 / 4096) as u8
            })
            .collect();
        Shadow { blur: blur.max(1), ramp }
    }

    /// 辺からの位置 `u`（辺の外側が負）の濃さ
    fn edge(&self, u: i32) -> u32 {
        let b = self.blur as i32;
        if u < -b {
            0
        } else if u >= b {
            255
        } else {
            self.ramp[(u + b) as usize] as u32
        }
    }

    /// 長さ `len` の辺に挟まれた区間の中の位置 `u` の濃さ（両側の縁の積）
    fn span(&self, u: i32, len: i32) -> u32 {
        self.edge(u).min(self.edge(len - 1 - u))
    }

    /// 矩形 (x, y, w, h) を `dy` 下にずらしたものの影の、(px, py) の濃さ（0〜255）
    pub fn alpha_at(&self, px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, dy: i32) -> u8 {
        let ax = self.span(px - x, w);
        let ay = self.span(py - (y + dy), h);
        (ax * ay / 255) as u8
    }
}

pub struct Canvas<'a> {
    width: usize,
    height: usize,
    pixels: &'a mut [Color],
}

impl<'a> Canvas<'a> {
    /// `pixels` は少なくとも `width * height` 個必要
    pub fn new(pixels: &'a mut [Color], width: usize, height: usize) -> Self {
        assert!(pixels.len() >= width * height, "canvas buffer too small");
        Canvas { width, height, pixels }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixels(&self) -> &[Color] {
        &self.pixels[..self.width * self.height]
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            None
        } else {
            Some(y as usize * self.width + x as usize)
        }
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Color> {
        self.index(x, y).map(|i| self.pixels[i])
    }

    pub fn put_pixel(&mut self, x: i32, y: i32, c: Color) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = c;
        }
    }

    /// (x, y) に `c` を濃さ `a` で重ねる
    pub fn blend_pixel(&mut self, x: i32, y: i32, c: Color, a: u8) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = match a {
                0 => return,
                255 => c,
                _ => blend(self.pixels[i], c, a),
            };
        }
    }

    pub fn clear(&mut self, c: Color) {
        let n = self.width * self.height;
        self.pixels[..n].fill(c);
    }

    /// 矩形を画面の中に切り詰める: (x0, y0, x1, y1)。空なら None
    fn clip(&self, x: i32, y: i32, w: i32, h: i32) -> Option<(usize, usize, usize, usize)> {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = x.saturating_add(w).min(self.width as i32);
        let y1 = y.saturating_add(h).min(self.height as i32);
        (x0 < x1 && y0 < y1).then_some((x0 as usize, y0 as usize, x1 as usize, y1 as usize))
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        for row in y0..y1 {
            let s = row * self.width;
            self.pixels[s + x0..s + x1].fill(c);
        }
    }

    /// 半透明の矩形
    pub fn fill_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color, a: u8) {
        if a == 255 {
            return self.fill_rect(x, y, w, h, c);
        }
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        for row in y0..y1 {
            let s = row * self.width;
            for p in &mut self.pixels[s + x0..s + x1] {
                *p = blend(*p, c, a);
            }
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, c: Color) {
        self.fill_rect(x, y, w, 1, c);
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, c: Color) {
        self.fill_rect(x, y, 1, h, c);
    }

    /// 枠だけの矩形
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.hline(x, y, w, c);
        self.hline(x, y + h - 1, w, c);
        self.vline(x, y, h, c);
        self.vline(x + w - 1, y, h, c);
    }

    /// 直線（Bresenham）。両端を含む
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.put_pixel(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// なめらかな太い線（端は丸い）。座標と太さは 1/16 ピクセル単位（ピクセルの真ん中は 8）。
    /// 各ピクセルの真ん中から線分までの距離で濃さを決める（整数だけ）
    pub fn line_aa(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, c: Color) {
        let half = width / 2;
        let m = half + 16;
        let Some((bx0, by0, bx1, by1)) =
            self.clip((x0.min(x1) - m) >> 4, (y0.min(y1) - m) >> 4, ((x0 - x1).abs() + 2 * m >> 4) + 2, ((y0 - y1).abs() + 2 * m >> 4) + 2)
        else {
            return;
        };
        let (dx, dy) = ((x1 - x0) as i64, (y1 - y0) as i64);
        let len2 = dx * dx + dy * dy;
        for py in by0..by1 {
            for px in bx0..bx1 {
                let (qx, qy) = ((px as i64) * 16 + 8 - x0 as i64, (py as i64) * 16 + 8 - y0 as i64);
                // 線分の上の一番近い点（t は 0〜len2）
                let t = if len2 == 0 { 0 } else { (qx * dx + qy * dy).clamp(0, len2) };
                let (ex, ey) = if len2 == 0 { (qx, qy) } else { (qx - dx * t / len2, qy - dy * t / len2) };
                let dist = isqrt((ex * ex + ey * ey) as u64) as i32;
                // 線の縁から外へ 1 ピクセル（16）で 255 → 0
                let cov = ((half + 8 - dist) * 16).clamp(0, 255);
                if cov > 0 {
                    self.blend_pixel(px as i32, py as i32, c, cov as u8);
                }
            }
        }
    }

    /// 縦のグラデーション（上端 `top` → 下端 `bottom`）
    pub fn gradient_v(&mut self, x: i32, y: i32, w: i32, h: i32, top: Color, bottom: Color) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        let n = (h - 1).max(0) as u32;
        for row in y0..y1 {
            let c = lerp(top, bottom, (row as i32 - y) as u32, n);
            let s = row * self.width;
            self.pixels[s + x0..s + x1].fill(c);
        }
    }

    /// 横のグラデーション（左端 `left` → 右端 `right`）
    pub fn gradient_h(&mut self, x: i32, y: i32, w: i32, h: i32, left: Color, right: Color) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        let n = (w - 1).max(0) as u32;
        for col in x0..x1 {
            let c = lerp(left, right, (col as i32 - x) as u32, n);
            for row in y0..y1 {
                self.pixels[row * self.width + col] = c;
            }
        }
    }

    /// 角の丸い矩形を塗る（角の縁は覆う割合で混ぜる）。`a` は全体の濃さ
    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, corner: &Corner, c: Color, a: u8) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        let r = (corner.r as i32).min(w / 2).min(h / 2);
        for row in y0..y1 {
            let ry = row as i32 - y;
            let s = row * self.width;
            if ry >= r && ry < h - r {
                // 角の無い行: 普通に塗る
                if a == 255 {
                    self.pixels[s + x0..s + x1].fill(c);
                } else {
                    for p in &mut self.pixels[s + x0..s + x1] {
                        *p = blend(*p, c, a);
                    }
                }
                continue;
            }
            for col in x0..x1 {
                let cov = corner.at(col as i32 - x, ry, w, h) as u32;
                let k = (cov * a as u32 / 255) as u8;
                let p = &mut self.pixels[s + col];
                *p = match k {
                    0 => *p,
                    255 => c,
                    _ => blend(*p, c, k),
                };
            }
        }
    }

    /// 角の丸い矩形の 1 ピクセルの枠
    pub fn round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, corner: &Corner, c: Color) {
        if w <= 0 || h <= 0 {
            return;
        }
        let r = (corner.r as i32).min(w / 2).min(h / 2);
        self.hline(x + r, y, w - 2 * r, c);
        self.hline(x + r, y + h - 1, w - 2 * r, c);
        self.vline(x, y + r, h - 2 * r, c);
        self.vline(x + w - 1, y + r, h - 2 * r, c);
        // 角: 外の円の覆う割合 − 1 ピクセル内側の円の覆う割合 を濃さにする
        for cy in 0..r {
            for cx in 0..r {
                let outer = corner.at(cx, cy, w, h) as i32;
                let inner = corner.at(cx - 1, cy - 1, w - 2, h - 2) as i32;
                let inner = if cx == 0 || cy == 0 { 0 } else { inner };
                let k = (outer - inner).clamp(0, 255) as u8;
                for (px, py) in [(cx, cy), (w - 1 - cx, cy), (cx, h - 1 - cy), (w - 1 - cx, h - 1 - cy)] {
                    self.blend_pixel(x + px, y + py, c, k);
                }
            }
        }
    }

    /// 矩形 (x, y, w, h) の影を、`dy` 下にずらして濃さ `a`（いちばん濃い所）で描く。矩形の中は描かない
    /// （上にウィンドウを重ねるので）
    pub fn shadow(&mut self, x: i32, y: i32, w: i32, h: i32, dy: i32, shadow: &Shadow, c: Color, a: u8) {
        let b = shadow.blur as i32;
        let (sx, sy, sw, sh) = (x - b, y + dy - b, w + 2 * b, h + 2 * b);
        let Some((x0, y0, x1, y1)) = self.clip(sx, sy, sw, sh) else { return };
        for row in y0..y1 {
            let py = row as i32;
            let inside_y = py >= y && py < y + h;
            let s = row * self.width;
            for col in x0..x1 {
                let px = col as i32;
                if inside_y && px >= x && px < x + w {
                    continue;
                }
                let k = shadow.alpha_at(px, py, x, y, w, h, dy) as u32 * a as u32 / 255;
                if k > 0 {
                    let p = &mut self.pixels[s + col];
                    *p = blend(*p, c, k as u8);
                }
            }
        }
    }

    /// 角の丸い矩形 (x, y, w, h) の影。`shadow` と同じだが、矩形の中でも角の外（丸めて欠けた所）には描く
    #[allow(clippy::too_many_arguments)]
    pub fn shadow_round(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        dy: i32,
        shadow: &Shadow,
        corner: &Corner,
        c: Color,
        a: u8,
    ) {
        let b = shadow.blur as i32;
        let (sx, sy, sw, sh) = (x - b, y + dy - b, w + 2 * b, h + 2 * b);
        let Some((x0, y0, x1, y1)) = self.clip(sx, sy, sw, sh) else { return };
        let r = (corner.r as i32).min(w / 2).min(h / 2);
        for row in y0..y1 {
            let py = row as i32;
            let ay = shadow.span(py - (y + dy), h);
            if ay == 0 {
                continue;
            }
            // 矩形が覆う列 [c0, c1)（角の行では角の間だけ）はまとめて飛ばす。残りの矩形の中は角の外だけ描く
            let ry = py - y;
            let (c0, c1) = match ry {
                _ if ry < 0 || ry >= h => (x0 as i32, x0 as i32),
                _ if ry >= r && ry < h - r => (x, x + w),
                _ => (x + r, x + w - r),
            };
            let (c0, c1) = (c0.clamp(x0 as i32, x1 as i32), c1.clamp(x0 as i32, x1 as i32));
            let c1 = c1.max(c0);
            let s = row * self.width;
            for col in (x0 as i32..c0).chain(c1..x1 as i32) {
                if ry >= 0 && ry < h && col >= x && col < x + w && corner.at(col - x, ry, w, h) == 255 {
                    continue;
                }
                let k = shadow.span(col - x, w) * ay / 255 * a as u32 / 255;
                if k > 0 {
                    let p = &mut self.pixels[s + col as usize];
                    *p = blend(*p, c, k as u8);
                }
            }
        }
    }

    /// 32 ビットの画像（幅 `src_w`）をそのまま転送する
    pub fn blit(&mut self, x: i32, y: i32, src: &[Color], src_w: usize) {
        if src_w == 0 {
            return;
        }
        let src_h = src.len() / src_w;
        let Some((x0, y0, x1, y1)) = self.clip(x, y, src_w as i32, src_h as i32) else { return };
        for row in y0..y1 {
            let sr = (row as i32 - y) as usize * src_w + (x0 as i32 - x) as usize;
            let d = row * self.width;
            self.pixels[d + x0..d + x1].copy_from_slice(&src[sr..sr + (x1 - x0)]);
        }
    }

    /// 8 ビットの画像（色番号、幅 `src_w`）を表 `lut` で 32 ビットに直して転送する
    pub fn blit8(&mut self, x: i32, y: i32, src: &[u8], src_w: usize, lut: &[Color; 256]) {
        if src_w == 0 {
            return;
        }
        let src_h = src.len() / src_w;
        let Some((x0, y0, x1, y1)) = self.clip(x, y, src_w as i32, src_h as i32) else { return };
        for row in y0..y1 {
            let sr = (row as i32 - y) as usize * src_w + (x0 as i32 - x) as usize;
            let d = row * self.width;
            for (p, &c) in self.pixels[d + x0..d + x1].iter_mut().zip(&src[sr..sr + (x1 - x0)]) {
                *p = lut[c as usize];
            }
        }
    }

    /// アルファ付きの画像（0xAARRGGBB、色にアルファを掛けてあるもの。幅 `src_w`）を重ねる
    pub fn blit_alpha(&mut self, x: i32, y: i32, src: &[u32], src_w: usize) {
        if src_w == 0 {
            return;
        }
        let src_h = src.len() / src_w;
        let Some((x0, y0, x1, y1)) = self.clip(x, y, src_w as i32, src_h as i32) else { return };
        for row in y0..y1 {
            let sr = (row as i32 - y) as usize * src_w + (x0 as i32 - x) as usize;
            let d = row * self.width;
            for (p, &s) in self.pixels[d + x0..d + x1].iter_mut().zip(&src[sr..sr + (x1 - x0)]) {
                let a = s >> 24;
                *p = match a {
                    0 => *p,
                    255 => s & 0x00FF_FFFF,
                    _ => (s & 0x00FF_FFFF) + scale(*p, 255 - a),
                };
            }
        }
    }

    /// `blit_alpha` と同じだが、画像全体の濃さを `fade / 255` にして重ねる（フォーカスの無いウィンドウのアイコン）
    pub fn blit_alpha_faded(&mut self, x: i32, y: i32, src: &[u32], src_w: usize, fade: u8) {
        if fade == 255 {
            return self.blit_alpha(x, y, src, src_w);
        }
        if src_w == 0 {
            return;
        }
        let src_h = src.len() / src_w;
        let Some((x0, y0, x1, y1)) = self.clip(x, y, src_w as i32, src_h as i32) else { return };
        for row in y0..y1 {
            let sr = (row as i32 - y) as usize * src_w + (x0 as i32 - x) as usize;
            let d = row * self.width;
            for (p, &s) in self.pixels[d + x0..d + x1].iter_mut().zip(&src[sr..sr + (x1 - x0)]) {
                // 事前に掛けたアルファの色ごと薄くする
                let a = ((s >> 24) * fade as u32 + 127) / 255;
                if a != 0 {
                    *p = scale(s & 0x00FF_FFFF, fade as u32) + scale(*p, 255 - a);
                }
            }
        }
    }

    /// 1 文字描く（ビットマップのフォント）。`bg` が `None` なら背景は描かない
    pub fn draw_char(&mut self, font: &Font, x: i32, y: i32, c: char, fg: Color, bg: Option<Color>) {
        for gy in 0..font.height {
            let bits = font.row_bits(c, gy);
            for gx in 0..font.width {
                match ((bits >> gx) & 1 != 0, bg) {
                    (true, _) => self.put_pixel(x + gx as i32, y + gy as i32, fg),
                    (false, Some(b)) => self.put_pixel(x + gx as i32, y + gy as i32, b),
                    (false, None) => {}
                }
            }
        }
    }

    /// 領域 (x, y, w, h) を `dy` ピクセル上へずらし、空いた下端を `fill` で塗る
    pub fn scroll_up(&mut self, x: i32, y: i32, w: i32, h: i32, dy: i32, fill: Color) {
        let Some((x0, y0, x1, y1)) = self.clip(x, y, w, h) else { return };
        if dy <= 0 {
            return;
        }
        let dy = (dy as usize).min(y1 - y0);
        for row in y0..y1 - dy {
            let dst = row * self.width;
            let src = (row + dy) * self.width;
            self.pixels.copy_within(src + x0..src + x1, dst + x0);
        }
        self.fill_rect(x0 as i32, (y1 - dy) as i32, (x1 - x0) as i32, dy as i32, fill);
    }

    /// なめらかなフォントで 1 文字描く（`pen_x` はペンの位置、`top` は行の上端）。送り幅（1/16 ピクセル）を返す
    pub fn draw_glyph(&mut self, font: &VFont, pen_x: i32, top: i32, c: char, color: Color) -> i32 {
        let Some(g) = font.glyph_or_replacement(c) else { return 0 };
        let gx = pen_x + g.left as i32;
        let gy = top + font.ascent as i32 + g.top as i32;
        for y in 0..g.height as usize {
            for x in 0..g.width as usize {
                let a = g.alpha(x, y);
                if a != 0 {
                    self.blend_pixel(gx + x as i32, gy + y as i32, color, a);
                }
            }
        }
        g.advance as i32
    }

    /// なめらかなフォントで文字列を 1 行描く（`top` は行の上端）。描いた幅を返す。
    /// ペンは 1/16 ピクセルで進め、字は丸めた位置に置く
    pub fn draw_vtext(&mut self, font: &VFont, x: i32, top: i32, text: &str, color: Color) -> i32 {
        let mut pen = 0i32;
        for c in text.chars() {
            let px = x + (pen + SUBPIXELS / 2) / SUBPIXELS;
            pen += self.draw_glyph(font, px, top, c, color);
        }
        (pen + SUBPIXELS / 2) / SUBPIXELS
    }

    /// 文字列を 1 行で描く。描いた幅を返す
    pub fn draw_text(&mut self, font: &Font, x: i32, y: i32, text: &str, fg: Color, bg: Option<Color>) -> i32 {
        let mut cx = x;
        for c in text.chars() {
            self.draw_char(font, cx, y, c, fg, bg);
            cx += font.width as i32;
        }
        cx - x
    }
}

/// 整数の平方根（切り捨て）
fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// 色を事前に掛けたアルファの 0xAARRGGBB にする（アイコン・カーソルを作るとき）
pub fn premultiply(c: Color, a: u8) -> u32 {
    (a as u32) << 24 | scale(c, a as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Color = 0x00FF_FFFF;
    const K: Color = 0;

    #[test]
    fn blend_endpoints_and_middle() {
        assert_eq!(blend(K, W, 0), K);
        assert_eq!(blend(K, W, 255), W);
        assert_eq!(blend(K, W, 128), rgb(128, 128, 128));
        assert_eq!(blend(rgb(200, 100, 0), rgb(0, 100, 200), 64), rgb(150, 100, 50));
        // 255 で割った結果がどの値でも正しく丸まる
        for s in [0u32, 1, 127, 128, 254, 255] {
            for a in 0..=255u32 {
                let want = (s * a + 255 * (255 - a) + 127) / 255;
                let got = blend(W, rgb(s as u8, s as u8, s as u8), a as u8) & 0xFF;
                assert_eq!(got, want, "s={s} a={a}");
            }
        }
    }

    #[test]
    fn lerp_and_gradient() {
        assert_eq!(lerp(K, W, 0, 4), K);
        assert_eq!(lerp(K, W, 4, 4), W);
        assert_eq!(lerp(rgb(0, 0, 100), rgb(100, 200, 0), 1, 2), rgb(50, 100, 50));
        let mut buf = [0u32; 3 * 5];
        let mut c = Canvas::new(&mut buf, 3, 5);
        c.gradient_v(0, 0, 3, 5, K, rgb(0, 0, 200));
        let col: Vec<u32> = (0..5).map(|y| c.get(1, y).unwrap()).collect();
        assert_eq!(col, [0, 50, 100, 150, 200]);
        c.gradient_h(0, 0, 3, 1, rgb(10, 0, 0), rgb(30, 0, 0));
        assert_eq!(&c.pixels()[..3], &[rgb(10, 0, 0), rgb(20, 0, 0), rgb(30, 0, 0)]);
    }

    #[test]
    fn fill_rect_clips_and_alpha() {
        let mut buf = [K; 16];
        let mut c = Canvas::new(&mut buf, 4, 4);
        c.fill_rect(-1, 2, 3, 5, W);
        assert_eq!(c.get(1, 3), Some(W));
        assert_eq!(c.get(2, 3), Some(K));
        assert_eq!(c.get(0, 1), Some(K));
        c.fill_rect_alpha(2, 0, 2, 2, W, 51);
        assert_eq!(c.get(3, 1), Some(rgb(51, 51, 51)));
    }

    #[test]
    fn corner_coverage_is_monotonic() {
        let k = Corner::new(6);
        // 外側の頂点はほぼ覆わない、内側（中心寄り）は全部覆う
        assert_eq!(k.coverage(0, 0), 0);
        assert_eq!(k.coverage(5, 5), 255);
        for y in 0..6 {
            for x in 1..6 {
                assert!(k.coverage(x, y) >= k.coverage(x - 1, y), "({x},{y})");
            }
        }
        // 対角線で対称
        for y in 0..6 {
            for x in 0..6 {
                assert_eq!(k.coverage(x, y), k.coverage(y, x));
            }
        }
    }

    #[test]
    fn round_rect_corners_are_symmetric() {
        let k = Corner::new(4);
        let mut buf = [K; 12 * 10];
        let mut c = Canvas::new(&mut buf, 12, 10);
        c.fill_round_rect(1, 1, 10, 8, &k, W, 255);
        // 角の外は塗らない、辺の真ん中は塗る
        assert_eq!(c.get(1, 1), Some(K));
        assert_eq!(c.get(6, 1), Some(W));
        assert_eq!(c.get(1, 5), Some(W));
        assert_eq!(c.get(0, 5), Some(K));
        // 4 つの角が鏡写し
        for y in 0..4 {
            for x in 0..4 {
                let tl = c.get(1 + x, 1 + y);
                assert_eq!(tl, c.get(10 - x, 1 + y), "({x},{y})");
                assert_eq!(tl, c.get(1 + x, 8 - y), "({x},{y})");
                assert_eq!(tl, c.get(10 - x, 8 - y), "({x},{y})");
            }
        }
        // 縁は途中の濃さになる（なめらか）
        let mid = (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).filter_map(|(x, y)| c.get(1 + x, 1 + y));
        assert!(mid.into_iter().any(|p| p != K && p != W));
    }

    #[test]
    fn round_rect_outline_stays_on_edge() {
        let k = Corner::new(3);
        let mut buf = [K; 10 * 8];
        let mut c = Canvas::new(&mut buf, 10, 8);
        c.round_rect(0, 0, 10, 8, &k, W);
        assert_eq!(c.get(5, 0), Some(W));
        assert_eq!(c.get(0, 4), Some(W));
        assert_eq!(c.get(5, 4), Some(K)); // 中は描かない
        assert_eq!(c.get(0, 0), Some(K)); // 角の外は描かない
    }

    #[test]
    fn shadow_fades_out_and_skips_inside() {
        let s = Shadow::new(4);
        let mut buf = [W; 30 * 30];
        let mut c = Canvas::new(&mut buf, 30, 30);
        c.shadow(10, 10, 10, 10, 2, &s, K, 255);
        // 中は描かない
        assert_eq!(c.get(15, 15), Some(W));
        // 下（ずらした分だけ濃い）は上より濃い。遠いほど薄い
        let below = c.get(15, 21).unwrap() & 0xFF;
        let above = c.get(15, 8).unwrap() & 0xFF;
        assert!(below < above, "below {below} above {above}");
        let far = c.get(15, 25).unwrap() & 0xFF;
        assert!(far > below);
        assert_eq!(c.get(15, 29), Some(W));
        // 左右対称
        assert_eq!(c.get(8, 15), c.get(21, 15));
    }

    /// 前の `shadow_round`（1 ピクセルずつ矩形の中かを調べる）
    #[allow(clippy::too_many_arguments)]
    fn shadow_round_reference(
        c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, dy: i32, shadow: &Shadow, corner: &Corner, col: Color, a: u8,
    ) {
        let b = shadow.blur as i32;
        let Some((x0, y0, x1, y1)) = c.clip(x - b, y + dy - b, w + 2 * b, h + 2 * b) else { return };
        for row in y0..y1 {
            let py = row as i32;
            let inside_y = py >= y && py < y + h;
            for px in x0 as i32..x1 as i32 {
                if inside_y && px >= x && px < x + w && corner.at(px - x, py - y, w, h) == 255 {
                    continue;
                }
                let k = shadow.alpha_at(px, py, x, y, w, h, dy) as u32 * a as u32 / 255;
                if k > 0 {
                    c.blend_pixel(px, py, col, k as u8);
                }
            }
        }
    }

    #[test]
    fn shadow_round_skips_columns_without_changing_pixels() {
        let corner = Corner::new(6);
        for blur in [1, 3, 10] {
            let s = Shadow::new(blur);
            // 画面の中・端からはみ出す・半径より小さい矩形・平たい矩形
            for (x, y, w, h, dy) in
                [(10, 10, 30, 20, 4), (-5, -8, 25, 30, 1), (30, 25, 8, 6, 0), (2, 15, 50, 3, 2), (45, 5, 20, 40, -3)]
            {
                let (mut a, mut b) = ([rgb(200, 210, 220); 60 * 50], [rgb(200, 210, 220); 60 * 50]);
                Canvas::new(&mut a, 60, 50).shadow_round(x, y, w, h, dy, &s, &corner, rgb(4, 0, 30), 180);
                shadow_round_reference(&mut Canvas::new(&mut b, 60, 50), x, y, w, h, dy, &s, &corner, rgb(4, 0, 30), 180);
                assert!(a == b, "blur {blur} rect ({x},{y},{w},{h}) dy {dy}");
            }
        }
    }

    #[test]
    fn blit_alpha_faded_halves_the_icon() {
        let mut buf = [K; 3];
        let mut c = Canvas::new(&mut buf, 3, 1);
        let src = [premultiply(W, 255), premultiply(W, 128), premultiply(W, 0)];
        c.blit_alpha_faded(0, 0, &src, 3, 128);
        assert_eq!(c.get(0, 0), Some(rgb(128, 128, 128)));
        assert_eq!(c.get(1, 0), Some(rgb(64, 64, 64)));
        assert_eq!(c.get(2, 0), Some(K));
        // 255 なら blit_alpha と同じ
        c.blit_alpha_faded(0, 0, &src, 3, 255);
        assert_eq!(c.get(0, 0), Some(W));
    }

    #[test]
    fn shadow_ramp_rises() {
        let s = Shadow::new(5);
        assert!(s.ramp.windows(2).all(|w| w[0] <= w[1]));
        assert!(s.ramp[0] < 10 && s.ramp[9] > 245);
        // 辺の上はおよそ半分（表は 1 ピクセルの真ん中で取るので少し濃い側）
        assert!((110..=150).contains(&s.edge(0)), "{}", s.edge(0));
    }

    #[test]
    fn blit_variants() {
        let mut buf = [K; 9];
        let mut c = Canvas::new(&mut buf, 3, 3);
        c.blit(1, 1, &[1, 2, 3, 4], 2);
        assert_eq!(c.pixels(), &[0, 0, 0, 0, 1, 2, 0, 3, 4]);
        let mut lut = [0u32; 256];
        lut[7] = W;
        c.blit8(-1, 0, &[7, 7], 2, &lut);
        assert_eq!(c.get(0, 0), Some(W));
        assert_eq!(c.get(1, 0), Some(K));
        // 半分の濃さの白（事前に掛けたもの）を黒の上に
        c.blit_alpha(2, 0, &[premultiply(W, 128)], 1);
        assert_eq!(c.get(2, 0), Some(rgb(128, 128, 128)));
        // 完全に透明・不透明
        c.blit_alpha(0, 2, &[premultiply(W, 0), premultiply(rgb(1, 2, 3), 255)], 2);
        assert_eq!(c.get(0, 2), Some(K));
        assert_eq!(c.get(1, 2), Some(rgb(1, 2, 3)));
    }

    #[test]
    fn palette8_conversion() {
        let p = palette::standard();
        assert_eq!(to_palette8(K), palette::GRAY_BASE);
        assert_eq!(p[to_palette8(W) as usize], [63, 63, 63]);
        assert_eq!(p[to_palette8(rgb(255, 0, 0)) as usize], [63, 0, 0]);
        let g = p[to_palette8(rgb(128, 128, 128)) as usize];
        assert!(g[0] == g[1] && (30..=34).contains(&g[0]), "{g:?}");
        // 少し温かい灰色もグレー（黄色に寄らない）
        let w = p[to_palette8(rgb(128, 128, 120)) as usize];
        assert!(w[0] == w[1] && w[1] == w[2], "{w:?}");
        assert_eq!(to_palette8(rgb(44, 44, 42)), to_palette8(rgb(43, 43, 43)));
    }

    #[test]
    fn vtext_blends_glyphs() {
        use vibe_font::vfn::build::{GlyphData, encode};
        let mut glyphs = vec![
            GlyphData { code: 'a' as u32, width: 2, height: 2, left: 0, top: -2, advance: 48, alpha: vec![15, 0, 0, 15] },
            GlyphData { code: '?' as u32, width: 1, height: 1, left: 0, top: -1, advance: 16, alpha: vec![15] },
        ];
        let data = encode(4, 4, 3, 0, &mut glyphs);
        let font = VFont::parse(&data).unwrap();
        let mut buf = [W; 8 * 4];
        let mut c = Canvas::new(&mut buf, 8, 4);
        // 行の上端 0、ベースライン 3: 字の上端は 1
        assert_eq!(c.draw_vtext(&font, 0, 0, "aa", K), 6);
        assert_eq!(c.get(0, 1), Some(K));
        assert_eq!(c.get(1, 1), Some(W));
        assert_eq!(c.get(1, 2), Some(K));
        assert_eq!(c.get(3, 1), Some(K)); // 2 文字目は 3 ピクセル目から
        assert_eq!(c.get(0, 0), Some(W));
    }

    #[test]
    fn isqrt_is_exact() {
        for n in [0u64, 1, 2, 3, 4, 15, 16, 17, 99, 100, 101, 1 << 40] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n} -> {r}");
        }
    }

    #[test]
    fn line_aa_is_solid_in_the_middle_and_soft_at_the_edge() {
        let mut buf = [W; 20 * 10];
        let mut c = Canvas::new(&mut buf, 20, 10);
        // (2.5, 5.5) → (17.5, 5.5)、太さ 2.5 ピクセル: 真ん中の行は塗りつぶし、上下の行は半分ほど
        c.line_aa(2 * 16 + 8, 5 * 16 + 8, 17 * 16 + 8, 5 * 16 + 8, 40, K);
        assert_eq!(c.get(10, 5), Some(K));
        let edge = c.get(10, 6).unwrap() & 0xFF;
        assert!(edge > 0 && edge < 255, "{edge}");
        // 上下対称、その外は描かない
        assert_eq!(c.get(10, 4), c.get(10, 6));
        assert_eq!(c.get(10, 7), Some(W));
        assert_eq!(c.get(10, 3), Some(W));
    }

    #[test]
    fn scroll_up_moves_rows() {
        let mut buf = [0u32; 6];
        let mut c = Canvas::new(&mut buf, 2, 3);
        for y in 0..3 {
            c.hline(0, y, 2, y as u32 + 1);
        }
        c.scroll_up(0, 0, 2, 3, 1, 9);
        assert_eq!(c.pixels(), &[2, 2, 3, 3, 9, 9]);
    }

    #[test]
    fn text_uses_font() {
        let mut buf = [K; 8 * 8];
        let mut c = Canvas::new(&mut buf, 8, 8);
        c.draw_char(&vibe_font::FONT_8X8, 0, 0, 'A', W, None);
        assert_eq!(c.get(2, 0), Some(W));
        assert_eq!(c.get(0, 0), Some(K));
    }
}
