//! 8bit パレット画像への描画。
//!
//! ハードウェアには触らず、バイト列（1 ピクセル = 1 バイト = パレット番号）に描くだけ。
//! はみ出した部分は切り捨てる（座標は負でもよい）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod bmp;
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
    fn scroll_up_moves_rows_and_fills() {
        let mut buf = [0u8; 12];
        let mut s = Surface::new(&mut buf, 3, 4);
        for y in 0..4 {
            s.hline(0, y, 3, y as u8 + 1);
        }
        s.scroll_up(0, 0, 3, 4, 1, 9);
        assert_eq!(ascii(&s), ["222", "333", "444", "999"]);
    }
}
