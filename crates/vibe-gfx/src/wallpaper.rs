//! デスクトップの背景の画像の並べ方と、BOOT.CFG の `wallpaper=` の読み方

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

/// 画面の `y` 行目（背景の画像の領域の中の行）に、`iw` × `ih` の画像（0x00RRGGBB）を並べた 1 行を書く。
/// `row` の長さが画面の幅 `sw`、背景の領域の高さが `sh`。真ん中に置くときに画像の無い所は書かない
/// （先に既定の背景を描いておく）。画面の大きさの画素を作らずに 1 行ずつ並べるので、大きな画面でもメモリを使わない
pub fn fill_row(row: &mut [u32], y: usize, img: &[u32], iw: usize, ih: usize, sh: usize, fit: Fit) {
    let sw = row.len();
    if iw == 0 || ih == 0 || img.len() < iw * ih {
        return;
    }
    match fit {
        Fit::Tile => {
            let src = &img[(y % ih) * iw..(y % ih + 1) * iw];
            for (x, px) in row.iter_mut().enumerate() {
                *px = src[x % iw];
            }
        }
        Fit::Center => {
            // 画像の方が大きければ画像の真ん中を切り出す
            let (ox, oy) = (sw as isize - iw as isize, sh as isize - ih as isize);
            let (dx, sx) = if ox >= 0 { (ox as usize / 2, 0) } else { (0, (-ox) as usize / 2) };
            let (dy, sy) = if oy >= 0 { (oy as usize / 2, 0) } else { (0, (-oy) as usize / 2) };
            let (w, h) = (iw.min(sw), ih.min(sh));
            if y >= dy && y < dy + h {
                let r = sy + y - dy;
                row[dx..dx + w].copy_from_slice(&img[r * iw + sx..r * iw + sx + w]);
            }
        }
    }
}

/// 並べ方の番号（システムコール wallpaper_set の引数）
pub fn fit_code(fit: Fit) -> u32 {
    match fit {
        Fit::Tile => 0,
        Fit::Center => 1,
    }
}

pub fn fit_from_code(code: u32) -> Option<Fit> {
    match code {
        0 => Some(Fit::Tile),
        1 => Some(Fit::Center),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

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

    fn compose(img: &[u32], iw: usize, ih: usize, sw: usize, sh: usize, fit: Fit, bg: u32) -> Vec<u32> {
        let mut out = vec![bg; sw * sh];
        for (y, row) in out.chunks_mut(sw).enumerate() {
            fill_row(row, y, img, iw, ih, sh, fit);
        }
        out
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
        let big: Vec<u32> = (0..16).collect();
        assert_eq!(compose(&big, 4, 4, 2, 2, Fit::Center, 0), [5, 6, 9, 10]);
        // 壊れた大きさなら何も書かない（背景のまま）
        assert_eq!(compose(&[1], 2, 2, 2, 1, Fit::Tile, 3), [3, 3]);
    }

    #[test]
    fn fit_codes_round_trip() {
        for f in [Fit::Tile, Fit::Center] {
            assert_eq!(fit_from_code(fit_code(f)), Some(f));
        }
        assert_eq!(fit_from_code(7), None);
    }
}
