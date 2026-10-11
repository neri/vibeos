//! ディスクに入れる生成物（テスト・デモ用の画像）を作る。
//!
//! 使い方: mkassets OUT_DIR
//! OUT_DIR の下に、ディスクのルートからの配置でファイルを書く。毎回同じ内容になる。

use std::fs;
use std::path::Path;

fn main() {
    let out = std::env::args().nth(1).expect("usage: mkassets OUT_DIR");
    let images = Path::new(&out).join("IMAGES");
    fs::create_dir_all(&images).unwrap();
    fs::write(images.join("PLASMA.BMP"), plasma()).unwrap();
    fs::write(images.join("GRADIENT.BMP"), gradient()).unwrap();
}

/// BMP を組み立てる（下の行から書く）。`palette` が空なら 24 ビット
fn bmp(width: u32, height: u32, palette: &[[u8; 3]], pixel: impl Fn(u32, u32) -> Vec<u8>) -> Vec<u8> {
    let bpp: u32 = if palette.is_empty() { 24 } else { 8 };
    let stride = (width * bpp / 8).next_multiple_of(4);
    let offset = 54 + palette.len() as u32 * 4;
    let size = offset + stride * height;
    let mut d = Vec::new();
    d.extend_from_slice(b"BM");
    d.extend_from_slice(&size.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    d.extend_from_slice(&offset.to_le_bytes());
    d.extend_from_slice(&40u32.to_le_bytes());
    d.extend_from_slice(&(width as i32).to_le_bytes());
    d.extend_from_slice(&(height as i32).to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&(bpp as u16).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // 無圧縮
    d.extend_from_slice(&(stride * height).to_le_bytes());
    d.extend_from_slice(&2835u32.to_le_bytes());
    d.extend_from_slice(&2835u32.to_le_bytes());
    d.extend_from_slice(&(palette.len() as u32).to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes());
    for c in palette {
        d.extend_from_slice(&[c[2], c[1], c[0], 0]);
    }
    for y in (0..height).rev() {
        let mut row = Vec::new();
        for x in 0..width {
            row.extend(pixel(x, y));
        }
        row.resize(stride as usize, 0);
        d.extend(row);
    }
    d
}

/// 8 ビット 320x200 のプラズマ模様（なめらかな独自パレット）
fn plasma() -> Vec<u8> {
    let palette: Vec<[u8; 3]> = (0..256)
        .map(|i| {
            let t = i as f64 / 256.0 * std::f64::consts::TAU;
            let c = |phase: f64| ((t + phase).sin() * 127.0 + 128.0) as u8;
            [c(0.0), c(2.1), c(4.2)]
        })
        .collect();
    bmp(320, 200, &palette, |x, y| {
        let (x, y) = (x as f64, y as f64);
        let v = (x / 16.0).sin() + (y / 8.0).sin() + ((x + y) / 16.0).sin() + ((x * x + y * y).sqrt() / 8.0).sin();
        vec![((v + 4.0) / 8.0 * 255.0) as u8]
    })
}

/// 24 ビット 160x100 のグラデーション
fn gradient() -> Vec<u8> {
    bmp(160, 100, &[], |x, y| {
        let r = (x * 255 / 159) as u8;
        let g = (y * 255 / 99) as u8;
        let b = 255 - r / 2 - g / 2;
        vec![b, g, r]
    })
}
