//! 色番号の画像への描画（1 ピクセル = 1 バイト = VGA の 16 色の番号。`palette::VGA16`）。
//!
//! ハードウェアには触らず、バイト列に描くだけ。
//! はみ出した部分は切り捨てる（座標は負でもよい）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod bmp;
pub mod dither;
pub mod icons;
pub mod mono;
pub mod palette;
pub mod wallpaper;

use vibe_font::Font;

pub struct Surface<'a> {
    width: usize,
    height: usize,
    pixels: &'a mut [u8],
}

impl<'a> Surface<'a> {
    /// `pixels` は少なくとも `width * height` バイト必要
    pub fn new(pixels: &'a mut [u8], width: usize, height: usize) -> Self {
        assert!(pixels.len() >= width * height, "surface buffer too small");
        Surface { width, height, pixels }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels[..self.width * self.height]
    }

    pub fn get(&self, x: i32, y: i32) -> Option<u8> {
        self.index(x, y).map(|i| self.pixels[i])
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            None
        } else {
            Some(y as usize * self.width + x as usize)
        }
    }

    pub fn put_pixel(&mut self, x: i32, y: i32, color: u8) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = color;
        }
    }

    pub fn clear(&mut self, color: u8) {
        let n = self.width * self.height;
        self.pixels[..n].fill(color);
    }

    /// 塗りつぶした矩形
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u8) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = x.saturating_add(w).min(self.width as i32);
        let y1 = y.saturating_add(h).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for row in y0..y1 {
            let start = row as usize * self.width;
            self.pixels[start + x0 as usize..start + x1 as usize].fill(color);
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u8) {
        self.fill_rect(x, y, w, 1, color);
    }

    pub fn vline(&mut self, x: i32, y: i32, h: i32, color: u8) {
        self.fill_rect(x, y, 1, h, color);
    }

    /// 枠だけの矩形
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u8) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.hline(x, y, w, color);
        self.hline(x, y + h - 1, w, color);
        self.vline(x, y, h, color);
        self.vline(x + w - 1, y, h, color);
    }

    /// 直線（Bresenham）。両端を含む
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u8) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.put_pixel(x, y, color);
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

    /// 立体の縁（1 ピクセル）: 上と左を `light`、下と右を `dark` で描く。出っ張りは明るい色を `light` に、
    /// へこみは暗い色を `light` に渡す
    pub fn bevel(&mut self, x: i32, y: i32, w: i32, h: i32, light: u8, dark: u8) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.hline(x, y, w - 1, light);
        self.vline(x, y, h - 1, light);
        self.hline(x, y + h - 1, w, dark);
        self.vline(x + w - 1, y, h, dark);
    }

    /// 円周（中点の円の描き方）
    pub fn circle(&mut self, cx: i32, cy: i32, r: i32, color: u8) {
        self.circle_octants(cx, cy, r, |s, x, y| {
            for (dx, dy) in [(x, y), (y, x), (-y, x), (-x, y), (-x, -y), (-y, -x), (y, -x), (x, -y)] {
                s.put_pixel(cx + dx, cy + dy, color);
            }
        });
    }

    /// 塗った円（`circle` と同じ縁の内側を全部塗る）
    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: u8) {
        self.circle_octants(cx, cy, r, |s, x, y| {
            s.hline(cx - x, cy + y, 2 * x + 1, color);
            s.hline(cx - x, cy - y, 2 * x + 1, color);
            s.hline(cx - y, cy + x, 2 * y + 1, color);
            s.hline(cx - y, cy - x, 2 * y + 1, color);
        });
    }

    /// 円の 1/8 の点 (x, y)（x >= y >= 0）を順に `f` に渡す
    fn circle_octants(&mut self, _cx: i32, _cy: i32, r: i32, mut f: impl FnMut(&mut Self, i32, i32)) {
        if r < 0 {
            return;
        }
        let (mut x, mut y, mut d) = (r, 0, 1 - r);
        while x >= y {
            f(self, x, y);
            y += 1;
            if d < 0 {
                d += 2 * y + 1;
            } else {
                x -= 1;
                d += 2 * (y - x) + 1;
            }
        }
    }

    /// 角の丸い矩形を塗る（角は半径 `r` の円）
    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: u8) {
        for j in 0..h.max(0) {
            if let Some((l, rr)) = round_span(w, h, r, j) {
                self.hline(x + l, y + j, rr - l, color);
            }
        }
    }

    /// 角の丸い矩形の太さ `t` の枠（中は塗らない）
    pub fn round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, t: i32, color: u8) {
        for j in 0..h.max(0) {
            let Some((l, rr)) = round_span(w, h, r, j) else { continue };
            let inner = if j >= t && j < h - t { round_span(w - 2 * t, h - 2 * t, (r - t).max(0), j - t) } else { None };
            match inner {
                Some((il, ir)) => {
                    self.hline(x + l, y + j, il + t - l, color);
                    self.hline(x + ir + t, y + j, rr - ir - t, color);
                }
                None => self.hline(x + l, y + j, rr - l, color),
            }
        }
    }

    /// 太さ `width` の直線（Bresenham の各点に `width` 四方の正方形を置く）
    pub fn thick_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, color: u8) {
        if width <= 1 {
            return self.line(x0, y0, x1, y1, color);
        }
        let half = (width - 1) / 2;
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.fill_rect(x - half, y - half, width, width, color);
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

    /// `src`（幅 `src_w`）を (x, y) に転送する。`transparent` の色は描かない
    pub fn blit(&mut self, x: i32, y: i32, src: &[u8], src_w: usize, transparent: Option<u8>) {
        if src_w == 0 {
            return;
        }
        for (sy, row) in src.chunks(src_w).enumerate() {
            for (sx, &c) in row.iter().enumerate() {
                if Some(c) != transparent {
                    self.put_pixel(x + sx as i32, y + sy as i32, c);
                }
            }
        }
    }

    /// 1 文字描く。`bg` が `None` なら背景は描かない
    pub fn draw_char(&mut self, font: &Font, x: i32, y: i32, c: char, fg: u8, bg: Option<u8>) {
        for gy in 0..font.height {
            let bits = font.row_bits(c, gy);
            for gx in 0..font.width {
                let on = (bits >> gx) & 1 != 0;
                match (on, bg) {
                    (true, _) => self.put_pixel(x + gx as i32, y + gy as i32, fg),
                    (false, Some(b)) => self.put_pixel(x + gx as i32, y + gy as i32, b),
                    (false, None) => {}
                }
            }
        }
    }

    /// 文字列を 1 行で描く（改行は扱わない）。描いた幅を返す
    pub fn draw_text(&mut self, font: &Font, x: i32, y: i32, text: &str, fg: u8, bg: Option<u8>) -> i32 {
        let mut cx = x;
        for c in text.chars() {
            self.draw_char(font, cx, y, c, fg, bg);
            cx += font.width as i32;
        }
        cx - x
    }

    /// 太字の文字列（1 ピクセル右にずらして重ねる。地は描かない）。送りはふつうの字と同じで、描いた幅を返す
    pub fn draw_text_bold(&mut self, font: &Font, x: i32, y: i32, text: &str, fg: u8) -> i32 {
        self.draw_text(font, x + 1, y, text, fg, None);
        self.draw_text(font, x, y, text, fg, None)
    }

    /// 領域 (x, y, w, h) を `dy` ピクセル上へずらし、空いた下端を `fill` で塗る
    pub fn scroll_up(&mut self, x: i32, y: i32, w: i32, h: i32, dy: i32, fill: u8) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).clamp(0, self.width as i32) as usize;
        let y1 = (y + h).clamp(0, self.height as i32) as usize;
        if x0 >= x1 || y0 >= y1 || dy <= 0 {
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
}

/// 幅 `w`・高さ `h`・角の半径 `r` の角の丸い矩形の `j` 行目に塗る x の範囲 [l, r)（左上からの位置）
fn round_span(w: i32, h: i32, r: i32, j: i32) -> Option<(i32, i32)> {
    if w <= 0 || h <= 0 || j < 0 || j >= h {
        return None;
    }
    let r = r.min(w / 2).min(h / 2).max(0);
    let dy = if j < r { r - j } else if j > h - 1 - r { j - (h - 1 - r) } else { 0 };
    // 角の円の中（中心からの距離の 2 乗が r² + 0.6r 以下）に入る横の広がり
    let limit = r * r + r * 6 / 10 - dy * dy;
    let mut k = r;
    while k > 0 && k * k > limit {
        k -= 1;
    }
    if limit < 0 {
        return None;
    }
    let inset = r - k;
    Some((inset, w - inset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibe_font::FONT_8X8;

    fn ascii(s: &Surface) -> Vec<String> {
        s.pixels()
            .chunks(s.width())
            .map(|r| r.iter().map(|&c| if c == 0 { '.' } else { char::from(b'0' + c) }).collect())
            .collect()
    }

    #[test]
    fn fill_rect_clips() {
        let mut buf = [0u8; 16];
        let mut s = Surface::new(&mut buf, 4, 4);
        s.fill_rect(-1, 2, 3, 5, 1);
        assert_eq!(ascii(&s), ["....", "....", "11..", "11.."]);
    }

    #[test]
    fn rect_outline() {
        let mut buf = [0u8; 20];
        let mut s = Surface::new(&mut buf, 5, 4);
        s.rect(0, 0, 5, 4, 2);
        assert_eq!(ascii(&s), ["22222", "2...2", "2...2", "22222"]);
    }

    #[test]
    fn line_diagonal_and_reverse() {
        let mut buf = [0u8; 16];
        let mut s = Surface::new(&mut buf, 4, 4);
        s.line(3, 3, 0, 0, 3);
        assert_eq!(ascii(&s), ["3...", ".3..", "..3.", "...3"]);
    }

    #[test]
    fn line_shallow() {
        let mut buf = [0u8; 15];
        let mut s = Surface::new(&mut buf, 5, 3);
        s.line(0, 0, 4, 2, 1);
        assert_eq!(ascii(&s), ["1....", ".11..", "...11"]);
    }

    #[test]
    fn blit_with_transparency() {
        let mut buf = [5u8; 9];
        let mut s = Surface::new(&mut buf, 3, 3);
        s.blit(1, 1, &[1, 0, 2, 3], 2, Some(0));
        assert_eq!(ascii(&s), ["555", "515", "523"]);
    }

    #[test]
    fn draw_char_matches_font() {
        let mut buf = [0u8; 64];
        let mut s = Surface::new(&mut buf, 8, 8);
        s.draw_char(&FONT_8X8, 0, 0, 'A', 1, Some(0));
        assert_eq!(ascii(&s)[0], "..11....");
        assert_eq!(ascii(&s)[4], "111111..");
    }

    #[test]
    fn draw_text_advances_by_font_width() {
        let mut buf = [0u8; 8 * 24];
        let mut s = Surface::new(&mut buf, 24, 8);
        assert_eq!(s.draw_text(&FONT_8X8, 0, 0, "AB", 1, None), 16);
        assert!(s.pixels()[16..24].iter().all(|&c| c == 0));
    }

    #[test]
    fn bevel_raised_and_sunken() {
        let mut buf = [0u8; 20];
        let mut s = Surface::new(&mut buf, 5, 4);
        s.bevel(0, 0, 5, 4, 7, 8);
        assert_eq!(ascii(&s), ["77778", "7...8", "7...8", "88888"]);
        s.bevel(1, 1, 3, 2, 8, 7);
        assert_eq!(ascii(&s), ["77778", "78878", "77778", "88888"]);
    }

    #[test]
    fn circle_and_fill_circle() {
        let mut buf = [0u8; 49];
        let mut s = Surface::new(&mut buf, 7, 7);
        s.circle(3, 3, 3, 1);
        assert_eq!(ascii(&s), ["..111..", ".1...1.", "1.....1", "1.....1", "1.....1", ".1...1.", "..111.."]);
        s.fill_circle(3, 3, 3, 2);
        assert_eq!(ascii(&s), ["..222..", ".22222.", "2222222", "2222222", "2222222", ".22222.", "..222.."]);
        // 半径 0 は 1 点、負は何も描かない
        let mut buf = [0u8; 9];
        let mut s = Surface::new(&mut buf, 3, 3);
        s.circle(1, 1, -1, 5);
        s.fill_circle(1, 1, 0, 4);
        assert_eq!(ascii(&s), ["...", ".4.", "..."]);
    }

    #[test]
    fn thick_line_is_wider() {
        let mut buf = [0u8; 30];
        let mut s = Surface::new(&mut buf, 6, 5);
        s.thick_line(1, 2, 4, 2, 3, 1);
        assert_eq!(ascii(&s), ["......", "111111", "111111", "111111", "......"]);
        let mut buf = [0u8; 30];
        let mut s = Surface::new(&mut buf, 6, 5);
        s.thick_line(1, 2, 4, 2, 1, 1);
        assert_eq!(ascii(&s), ["......", "......", ".1111.", "......", "......"]);
    }

    #[test]
    fn bold_text_overstrikes_one_pixel_right() {
        let mut buf = [0u8; 64];
        let mut s = Surface::new(&mut buf, 8, 8);
        assert_eq!(s.draw_text_bold(&FONT_8X8, 0, 0, "A", 1), 8);
        assert_eq!(ascii(&s)[0], "..111...");
        assert_eq!(ascii(&s)[4], "1111111.");
    }

    #[test]
    fn scroll_up_moves_rows_and_fills() {
        let mut buf = [0u8; 12];
        let mut s = Surface::new(&mut buf, 3, 4);
        for y in 0..4 {
            s.hline(0, y, 3, y as u8 + 1);
        }
        s.scroll_up(0, 0, 3, 4, 1, 9);
        assert_eq!(ascii(&s), ["222", "333", "444", "999"]);
    }

    #[test]
    fn fill_round_rect_cuts_the_corners() {
        let mut buf = [0u8; 64];
        let mut s = Surface::new(&mut buf, 8, 8);
        s.fill_round_rect(0, 0, 8, 6, 2, 1);
        assert_eq!(
            ascii(&s),
            [".111111.", "11111111", "11111111", "11111111", "11111111", ".111111.", "........", "........"]
        );
    }

    #[test]
    fn round_rect_is_hollow() {
        let mut buf = [0u8; 64];
        let mut s = Surface::new(&mut buf, 8, 8);
        s.round_rect(0, 0, 8, 7, 2, 2, 1);
        assert_eq!(
            ascii(&s),
            [".111111.", "11111111", "11....11", "11....11", "11....11", "11111111", ".111111.", "........"]
        );
    }

    #[test]
    fn round_rect_with_zero_radius_is_a_rect() {
        let mut buf = [0u8; 16];
        let mut s = Surface::new(&mut buf, 4, 4);
        s.round_rect(0, 0, 4, 4, 0, 1, 1);
        assert_eq!(ascii(&s), ["1111", "1..1", "1..1", "1111"]);
    }
}
