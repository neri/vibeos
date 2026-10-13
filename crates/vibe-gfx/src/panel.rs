//! 角の丸いパネル（ウィンドウ・メニュー・ドック）を 1 行ずつ重ねる
//!
//! 重ね合わせは 1 行ずつなので、パネルも 1 行ずつ描く（その行だけの高さ 1 の `Canvas` に、
//! y をずらした座標で描くと、範囲の切り詰めがその行だけを残す）。順番は、影（遠い層 → 近い層）→ 中身 →
//! 縁（あれば）→ 角を覆う割合で下の色と混ぜる。角の所の下の色は、中身を描く前に取っておく。

use crate::canvas::{Canvas, Color, Corner, Shadow, blend};

/// `panel_row` で使える角の半径の上限（角の所の下の色を取っておく配列の大きさ）
pub const MAX_RADIUS: usize = 24;

/// 影の 1 層: ぼかしの表・下へのずれ・いちばん濃い所の濃さ（255 で `color` そのもの）
#[derive(Clone, Copy)]
pub struct ShadowLayer<'a> {
    pub shadow: &'a Shadow,
    pub dy: i32,
    pub alpha: u8,
}

/// パネルの見た目: 角・影（先に描くものから）・影の色・1 ピクセルの縁（None なら描かない）
pub struct PanelStyle<'a> {
    pub corner: &'a Corner,
    pub shadows: &'a [ShadowLayer<'a>],
    pub shadow_color: Color,
    pub border: Option<Color>,
}

/// 角の丸いパネル `rect` = (x, y, w, h) の `y` 行目を `row`（画面の 1 行）に重ねる。
/// `fill` は中身を `row` の範囲 `[x0, x1)` に書く（引数は x0 と、その範囲の行）。
/// 縁があれば矩形のいちばん外側の 1 ピクセルに描く（中身の上に）
pub fn panel_row(
    row: &mut [u32],
    y: i32,
    rect: (i32, i32, i32, i32),
    style: &PanelStyle,
    fill: impl FnOnce(i32, &mut [u32]),
) {
    let (px, py, pw, ph) = rect;
    let width = row.len() as i32;
    let corner = style.corner;
    assert!(corner.r <= MAX_RADIUS, "panel corner too large");
    if !style.shadows.is_empty() {
        let mut c = Canvas::new(row, width as usize, 1);
        for l in style.shadows {
            c.shadow_round(px, py - y, pw, ph, l.dy, l.shadow, corner, style.shadow_color, l.alpha);
        }
    }
    if y < py || y >= py + ph || pw <= 0 {
        return;
    }
    let (x0, x1) = (px.max(0), (px + pw).min(width));
    if x0 >= x1 {
        return;
    }
    let dy = y - py;
    let r = (corner.r as i32).min(pw / 2).min(ph / 2);
    let corner_row = dy < r || dy >= ph - r;
    // 角の所の下の色を取っておく（左右それぞれ r ピクセルまで）
    let mut below = [0u32; 2 * MAX_RADIUS];
    let zone = |i: i32| -> Option<i32> {
        // 角の i 番目のピクセルの x（左の r 個、右の r 個）
        let x = if i < r { px + i } else { px + pw - 2 * r + i };
        (x >= x0 && x < x1).then_some(x)
    };
    if corner_row {
        for i in 0..2 * r {
            if let Some(x) = zone(i) {
                below[i as usize] = row[x as usize];
            }
        }
    }
    fill(x0, &mut row[x0 as usize..x1 as usize]);
    if let Some(border) = style.border {
        if dy == 0 || dy == ph - 1 {
            row[x0 as usize..x1 as usize].fill(border);
        } else {
            for x in [px, px + pw - 1] {
                if x >= x0 && x < x1 {
                    row[x as usize] = border;
                }
            }
        }
    }
    if !corner_row {
        return;
    }
    for i in 0..2 * r {
        let Some(x) = zone(i) else { continue };
        let dx = x - px;
        let outer = corner.at(dx, dy, pw, ph);
        let inside = row[x as usize];
        let c = match style.border {
            // 縁: 1 ピクセル内側の角の外は縁の色
            Some(border) => {
                let inner = if dx >= 1 && dy >= 1 { corner.at(dx - 1, dy - 1, pw - 2, ph - 2) } else { 0 };
                blend(border, inside, inner)
            }
            None => inside,
        };
        row[x as usize] = blend(below[i as usize], c, outer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::rgb;

    const RED: Color = rgb(255, 0, 0);
    const WHITE: Color = rgb(255, 255, 255);
    const BLUE: Color = rgb(0, 0, 255);

    /// 幅 40 の行に (5, 0, 30, 20) のパネルを 1 行ずつ描いた 40x20 の画像
    fn render(style: &PanelStyle, bg: Color) -> Vec<u32> {
        let (w, h) = (40usize, 20usize);
        let mut img = vec![bg; w * h];
        for y in 0..h {
            let row = &mut img[y * w..(y + 1) * w];
            panel_row(row, y as i32, (5, 0, 30, 20), style, |_, span| span.fill(WHITE));
        }
        img
    }

    #[test]
    fn borderless_corner_mixes_only_fill_and_below() {
        let corner = Corner::new(6);
        let style = PanelStyle { corner: &corner, shadows: &[], shadow_color: 0, border: None };
        let img = render(&style, RED);
        for y in 0..20 {
            for x in 0..40 {
                let p = img[y * 40 + x];
                let cov = corner.at(x as i32 - 5, y as i32, 30, 20);
                assert_eq!(p, blend(RED, WHITE, cov), "({x},{y})");
            }
        }
        // 角の頂点は下の色、辺の真ん中は中身
        assert_eq!(img[5], RED);
        assert_eq!(img[20], WHITE);
        assert_eq!(img[10 * 40 + 5], WHITE);
    }

    #[test]
    fn border_is_drawn_when_given() {
        let corner = Corner::new(6);
        let style = PanelStyle { corner: &corner, shadows: &[], shadow_color: 0, border: Some(BLUE) };
        let img = render(&style, RED);
        assert_eq!(img[20], BLUE); // 上の辺
        assert_eq!(img[10 * 40 + 5], BLUE); // 左の辺
        assert_eq!(img[10 * 40 + 6], WHITE);
    }

    #[test]
    fn two_layer_shadow_fades_outward_and_stays_outside() {
        let corner = Corner::new(6);
        let (near, far) = (Shadow::new(3), Shadow::new(10));
        let shadows = [
            ShadowLayer { shadow: &far, dy: 5, alpha: 128 },
            ShadowLayer { shadow: &near, dy: 1, alpha: 64 },
        ];
        let style = PanelStyle { corner: &corner, shadows: &shadows, shadow_color: 0, border: None };
        let (w, h) = (60usize, 60usize);
        let mut img = vec![WHITE; w * h];
        let rect = (15, 10, 30, 30);
        for y in 0..h {
            // 中身は描かない（影が中身の下に描かれていないことを見る）
            panel_row(&mut img[y * w..(y + 1) * w], y as i32, rect, &style, |_, _| {});
        }
        let at = |x: usize, y: usize| img[y * w + x] & 0xFF;
        // 中身の下（角の外でない所）は白のまま
        for y in 10..40 {
            for x in 15..45 {
                if corner.at(x as i32 - 15, y as i32 - 10, 30, 30) == 255 {
                    assert_eq!(img[y * w + x], WHITE, "({x},{y})");
                }
            }
        }
        // 下へ離れるほど薄い（明るい）
        let below: Vec<u32> = (40..56).map(|y| at(30, y)).collect();
        assert!(below.windows(2).all(|p| p[0] <= p[1]), "{below:?}");
        assert!(below[0] < 200 && below[15] == 255, "{below:?}");
        // 下は上より濃い（下へずらしてある）
        assert!(at(30, 40) < at(30, 9));
        // 左右対称
        for y in 0..h {
            assert_eq!(at(14, y), at(45, y), "y={y}");
        }
    }
}
