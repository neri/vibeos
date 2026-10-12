//! デスクトップの背景の画像の並べ方と、BOOT.CFG の `wallpaper=` の読み方

use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// 真ん中に 1 枚（はみ出す分は切る）
    Center,
    /// 左上から敷き詰める
    Tile,
}

/// "PATH [tile|center]" を (PATH, 並べ方) にする。並べ方を書かなければ真ん中
pub fn parse_setting(value: &str) -> Option<(&str, Fit)> {
    let mut words = value.split_whitespace();
    let path = words.next()?;
    let fit = match words.next() {
        None | Some("center") => Fit::Center,
        Some("tile") => Fit::Tile,
        Some(_) => return None,
    };
    words.next().is_none().then_some((path, fit))
}

/// BOOT.CFG の文から `wallpaper=` の値（"#" より後ろはコメント）
pub fn from_config(text: &str) -> Option<&str> {
    text.lines().filter_map(|l| l.split('#').next()?.split_once('=')).find(|(k, _)| k.trim() == "wallpaper").map(|(_, v)| v.trim())
}

/// `iw` × `ih` の画像（標準パレットの番号）を `sw` × `sh` に並べた画素。空いた所は `bg`
pub fn compose(img: &[u8], iw: usize, ih: usize, sw: usize, sh: usize, fit: Fit, bg: u8) -> Vec<u8> {
    let mut out = vec![bg; sw * sh];
    if iw == 0 || ih == 0 || img.len() < iw * ih {
        return out;
    }
    match fit {
        Fit::Tile => {
            for y in 0..sh {
                let src = &img[(y % ih) * iw..(y % ih + 1) * iw];
                for (x, px) in out[y * sw..(y + 1) * sw].iter_mut().enumerate() {
                    *px = src[x % iw];
                }
            }
        }
        Fit::Center => {
            // 画像の方が大きければ画像の真ん中を切り出す
            let (ox, oy) = (sw as isize - iw as isize, sh as isize - ih as isize);
            let (dx, sx) = if ox >= 0 { (ox as usize / 2, 0) } else { (0, (-ox) as usize / 2) };
            let (dy, sy) = if oy >= 0 { (oy as usize / 2, 0) } else { (0, (-oy) as usize / 2) };
            let (w, h) = (iw.min(sw), ih.min(sh));
            for y in 0..h {
                let src = &img[(sy + y) * iw + sx..(sy + y) * iw + sx + w];
                out[(dy + y) * sw + dx..(dy + y) * sw + dx + w].copy_from_slice(src);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings() {
        assert_eq!(parse_setting("/IMAGES/A.BMP"), Some(("/IMAGES/A.BMP", Fit::Center)));
        assert_eq!(parse_setting(" /A.BMP  tile "), Some(("/A.BMP", Fit::Tile)));
        assert_eq!(parse_setting("/A.BMP stretch"), None);
        assert_eq!(parse_setting("/A.BMP tile x"), None);
        assert_eq!(parse_setting(""), None);
        let cfg = "video=640x480\n# wallpaper=/NO.BMP\nwallpaper = /IMAGES/P.BMP tile # comment\n";
        assert_eq!(from_config(cfg), Some("/IMAGES/P.BMP tile"));
        assert_eq!(from_config("video=640x480\n"), None);
    }

    #[test]
    fn tile_repeats_the_image() {
        let img = [1, 2, 3, 4]; // 2x2
        let out = compose(&img, 2, 2, 5, 3, Fit::Tile, 0);
        assert_eq!(out, [1, 2, 1, 2, 1, 3, 4, 3, 4, 3, 1, 2, 1, 2, 1]);
    }

    #[test]
    fn center_small_and_large_images() {
        let img = [7; 4]; // 2x2
        let out = compose(&img, 2, 2, 4, 4, Fit::Center, 9);
        assert_eq!(out, [9, 9, 9, 9, 9, 7, 7, 9, 9, 7, 7, 9, 9, 9, 9, 9]);
        // 4x4 の画像を 2x2 に: 真ん中を切り出す
        let big: Vec<u8> = (0..16).collect();
        assert_eq!(compose(&big, 4, 4, 2, 2, Fit::Center, 0), [5, 6, 9, 10]);
        // 壊れた大きさなら背景だけ
        assert_eq!(compose(&[1], 2, 2, 2, 1, Fit::Tile, 3), [3, 3]);
    }
}
