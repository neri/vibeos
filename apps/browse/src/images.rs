//! ページの中の画像（PNG と BMP）
//!
//! 中身の先頭で形式を見分ける（PNG の印・"BM"）。1 枚は 1024x1024 まで。透明な所はページの地の色にする。
//! 表示する大きさは `<img>` の width・height（片方だけなら縦横の比を保つ）、無ければ画像の大きさ。

use alloc::vec::Vec;

use vibe_rt::gfx_lib::bmp::Bmp;
use vibe_web::doc;
use vibe_web::png;

use crate::view::PAGE_BG;

/// 1 枚の大きさの上限（1 辺）
pub const MAX_SIDE: usize = 1024;

pub struct Picture {
    pub w: usize,
    pub h: usize,
    /// 0x00RRGGBB
    pub pixels: Vec<u32>,
}

/// 画像のファイルを読む
pub fn decode(data: &[u8]) -> Result<Picture, &'static str> {
    if png::is_png(data) {
        let img = png::decode(data, MAX_SIDE * MAX_SIDE, PAGE_BG).map_err(|e| e.message())?;
        if img.width > MAX_SIDE || img.height > MAX_SIDE {
            return Err("image too large");
        }
        return Ok(Picture { w: img.width, h: img.height, pixels: img.pixels });
    }
    if data.starts_with(b"BM") {
        let bmp = Bmp::parse(data).map_err(|_| "broken BMP file")?;
        let (w, h) = (bmp.width as usize, bmp.height as usize);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
            return Err("image too large");
        }
        return Ok(Picture { w, h, pixels: bmp.to_rgb32() });
    }
    Err("not a PNG or BMP image")
}

/// 表示する大きさ
pub fn display_size(img: &doc::Image, p: &Picture) -> (u32, u32) {
    let (w, h) = (p.w as u32, p.h as u32);
    let (dw, dh) = match (img.width, img.height) {
        (Some(dw), Some(dh)) => (dw, dh),
        (Some(dw), None) => (dw, (h as u64 * dw as u64 / w.max(1) as u64) as u32),
        (None, Some(dh)) => ((w as u64 * dh as u64 / h.max(1) as u64) as u32, dh),
        (None, None) => (w, h),
    };
    (dw.clamp(1, 4096), dh.clamp(1, 4096))
}
