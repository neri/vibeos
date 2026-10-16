//! BMP 画像の読み込み（無圧縮の 8 / 24 / 32 ビット）

use alloc::vec;
use alloc::vec::Vec;

use crate::palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmpError {
    NotBmp,
    Unsupported(&'static str),
    Truncated,
}

impl core::fmt::Display for BmpError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            BmpError::NotBmp => write!(f, "not a BMP file"),
            BmpError::Unsupported(what) => write!(f, "unsupported BMP: {what}"),
            BmpError::Truncated => write!(f, "BMP file is truncated"),
        }
    }
}

pub struct Bmp<'a> {
    pub width: u32,
    pub height: u32,
    bits: u16,
    top_down: bool,
    stride: usize,
    pixels: &'a [u8],
    /// 8 ビットのときのパレット（8 ビット RGB）
    colors: Vec<[u8; 3]>,
}

fn le16(b: &[u8], o: usize) -> u32 {
    u16::from_le_bytes([b[o], b[o + 1]]) as u32
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl<'a> Bmp<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Bmp<'a>, BmpError> {
        if data.len() < 54 || &data[0..2] != b"BM" {
            return Err(BmpError::NotBmp);
        }
        let offset = le32(data, 10) as usize;
        let header_size = le32(data, 14) as usize;
        if header_size < 40 {
            return Err(BmpError::Unsupported("old header"));
        }
        let width = le32(data, 18) as i32;
        let height = le32(data, 22) as i32;
        let bits = le16(data, 28) as u16;
        let compression = le32(data, 30);
        if compression != 0 && !(compression == 3 && bits == 32) {
            return Err(BmpError::Unsupported("compressed"));
        }
        if !matches!(bits, 8 | 24 | 32) {
            return Err(BmpError::Unsupported("bit depth"));
        }
        if width <= 0 || height == 0 || width > 4096 || height.unsigned_abs() > 4096 {
            return Err(BmpError::Unsupported("size"));
        }
        let (width, top_down) = (width as u32, height < 0);
        let height = height.unsigned_abs();
        let stride = (width as usize * bits as usize / 8).next_multiple_of(4);
        let end = offset.checked_add(stride * height as usize).ok_or(BmpError::Truncated)?;
        if end > data.len() {
            return Err(BmpError::Truncated);
        }
        let mut colors = Vec::new();
        if bits == 8 {
            let n = match le32(data, 46) {
                0 => 256,
                n => n.min(256) as usize,
            };
            let table = 14 + header_size;
            if table + n * 4 > offset {
                return Err(BmpError::Truncated);
            }
            for i in 0..n {
                let e = table + i * 4;
                colors.push([data[e + 2], data[e + 1], data[e]]); // BGR0 → RGB
            }
        }
        Ok(Bmp { width, height, bits, top_down, stride, pixels: &data[offset..end], colors })
    }

    /// 8 ビット（パレット付き）か
    pub fn is_indexed(&self) -> bool {
        self.bits == 8
    }

    /// 8 ビットのときのパレット（8 ビット RGB）
    pub fn colors(&self) -> &[[u8; 3]] {
        &self.colors
    }

    fn row(&self, y: u32) -> &[u8] {
        let r = if self.top_down { y } else { self.height - 1 - y } as usize;
        &self.pixels[r * self.stride..(r + 1) * self.stride]
    }

    /// 8 ビットのときのパレット番号
    pub fn index(&self, x: u32, y: u32) -> u8 {
        debug_assert!(self.is_indexed());
        self.row(y)[x as usize]
    }

    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let row = self.row(y);
        match self.bits {
            8 => self.colors.get(row[x as usize] as usize).copied().unwrap_or([0, 0, 0]),
            _ => {
                let o = x as usize * (self.bits as usize / 8);
                [row[o + 2], row[o + 1], row[o]]
            }
        }
    }

    /// 32 ビットの色（0x00RRGGBB）の画素（左上から）。8 ビットの画像も画像のパレットの色そのまま
    pub fn to_rgb32(&self) -> Vec<u32> {
        let (w, h) = (self.width, self.height);
        let mut out = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = self.rgb(x, y);
                out.push((r as u32) << 16 | (g as u32) << 8 | b as u32);
            }
        }
        out
    }

    /// VGA の 16 色（`palette::VGA16`）の番号に変換した画素（左上から）。8 ビットの画像も含めて誤差拡散する
    /// （16 色では色ごとの置き換えだけだと段差が大きいため）
    pub fn to_vga16(&self, lut: &crate::dither::Lut16) -> Vec<u8> {
        crate::dither::to_vga16(self.width as usize, self.height as usize, lut, |x, y| self.rgb(x as u32, y as u32))
    }

    /// 標準パレットの番号に変換した画素（左上から）。
    /// 8 ビットは色ごとに近い色へ置き換え、24/32 ビットは誤差拡散（Floyd–Steinberg）で階調を表す
    pub fn to_standard_palette(&self) -> Vec<u8> {
        let pal = palette::standard();
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = Vec::with_capacity(w * h);
        if self.is_indexed() {
            // パレットの色ごとに 1 回だけ近い色を探す
            let map: Vec<u8> = self.colors.iter().map(|&[r, g, b]| palette::nearest(&pal, r, g, b)).collect();
            for y in 0..self.height {
                for x in 0..self.width {
                    out.push(map.get(self.index(x, y) as usize).copied().unwrap_or(0));
                }
            }
            return out;
        }
        // 今の行と次の行に持ち越す誤差（RGB, 8bit の単位 × 16）
        let mut err = vec![[0i32; 3]; w + 2];
        let mut next = vec![[0i32; 3]; w + 2];
        for y in 0..self.height {
            for x in 0..w {
                let src = self.rgb(x as u32, y);
                let want: [i32; 3] = core::array::from_fn(|i| (src[i] as i32 + err[x + 1][i] / 16).clamp(0, 255));
                let idx = palette::nearest(&pal, want[0] as u8, want[1] as u8, want[2] as u8);
                out.push(idx);
                let got = pal[idx as usize];
                for i in 0..3 {
                    // パレットは 6bit なので 8bit に戻して差を取る
                    let e = want[i] - (got[i] as i32 * 255 / 63);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用に BMP を組み立てる
    fn make(width: i32, height: i32, bits: u16, palette: &[[u8; 3]], rows: &[Vec<u8>]) -> Vec<u8> {
        let stride = (width as usize * bits as usize / 8).next_multiple_of(4);
        let offset = 54 + palette.len() * 4;
        let mut d = Vec::new();
        d.extend_from_slice(b"BM");
        d.extend_from_slice(&((offset + stride * rows.len()) as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&(offset as u32).to_le_bytes());
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&width.to_le_bytes());
        d.extend_from_slice(&height.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&bits.to_le_bytes());
        d.extend_from_slice(&[0; 4]); // 無圧縮
        d.extend_from_slice(&[0; 12]);
        d.extend_from_slice(&(palette.len() as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        for c in palette {
            d.extend_from_slice(&[c[2], c[1], c[0], 0]);
        }
        for r in rows {
            let mut r = r.clone();
            r.resize(stride, 0);
            d.extend_from_slice(&r);
        }
        d
    }

    #[test]
    fn indexed_bottom_up() {
        let pal = [[0, 0, 0], [255, 0, 0], [0, 0, 255]];
        // ファイル上は下の行が先
        let data = make(3, 2, 8, &pal, &[vec![2, 2, 2], vec![0, 1, 2]]);
        let b = Bmp::parse(&data).unwrap();
        assert_eq!((b.width, b.height, b.is_indexed()), (3, 2, true));
        assert_eq!(b.index(1, 0), 1);
        assert_eq!(b.rgb(1, 0), [255, 0, 0]);
        assert_eq!(b.rgb(0, 1), [0, 0, 255]);
        let std = b.to_standard_palette();
        let p = palette::standard();
        assert_eq!(p[std[1] as usize], [63, 0, 0]);
        assert_eq!(std[0], palette::BLACK);
    }

    #[test]
    fn rgb24_top_down_with_padding() {
        // 幅 1 → 1 行 3 バイト + 詰め物 1 バイト
        let data = make(1, -2, 24, &[], &[vec![0, 0, 255], vec![255, 255, 255]]);
        let b = Bmp::parse(&data).unwrap();
        assert_eq!(b.rgb(0, 0), [255, 0, 0]);
        assert_eq!(b.rgb(0, 1), [255, 255, 255]);
        assert_eq!(b.to_standard_palette()[1], palette::WHITE);
    }

    #[test]
    fn rejects_bad_files() {
        assert_eq!(Bmp::parse(b"hello").err(), Some(BmpError::NotBmp));
        let mut data = make(4, 4, 24, &[], &vec![vec![0; 12]; 4]);
        data.truncate(data.len() - 1);
        assert_eq!(Bmp::parse(&data).err(), Some(BmpError::Truncated));
        let mut data = make(4, 4, 24, &[], &vec![vec![0; 12]; 4]);
        data[28] = 4; // 4 ビット
        assert_eq!(Bmp::parse(&data).err(), Some(BmpError::Unsupported("bit depth")));
    }
}
