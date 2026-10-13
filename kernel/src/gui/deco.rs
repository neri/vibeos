//! デスクトップの飾り: 角の丸いウィンドウ・パネル（メニュー）の縁と影、アルファ付きのマウスカーソル
//!
//! 重ね合わせは 1 行ずつなので、ここも 1 行ずつ描く（その行だけの高さ 1 の `Canvas` に、
//! y をずらした座標で描くと、範囲の切り詰めがその行だけを残す）。角の丸みと影の表（`Corner`・`Shadow`）は
//! デスクトップにするときに 1 回だけ作る。

use alloc::vec;
use alloc::vec::Vec;

use vibe_gfx::canvas::{Canvas, Corner, Shadow, blend, premultiply, rgb};
use vibe_gfx::theme;

/// マウスカーソルの画像の大きさ（矢印の先が左上 (0, 0)）
pub(super) const CURSOR_W: usize = 16;
pub(super) const CURSOR_H: usize = 23;

pub(super) struct Deco {
    pub(super) corner: Corner,
    pub(super) corner_small: Corner,
    pub(super) shadow: Shadow,
    /// 事前に掛けたアルファの 0xAARRGGBB（CURSOR_W × CURSOR_H）
    pub(super) cursor: Vec<u32>,
}

impl Deco {
    pub(super) fn new() -> Deco {
        Deco {
            corner: Corner::new(theme::RADIUS),
            corner_small: Corner::new(theme::RADIUS_SMALL),
            shadow: Shadow::new(theme::SHADOW_BLUR),
            cursor: build_cursor(),
        }
    }
}

/// 影がウィンドウの外へはみ出す幅（描き直す行を広げるのに使う）
pub(super) const SHADOW_MARGIN: i32 = theme::SHADOW_BLUR as i32 + theme::SHADOW_DY;

/// 角の丸いパネル（ウィンドウの枠を含む矩形 `rect` = (x, y, w, h)）の `y` 行目を `row` に重ねる。
/// `shadow` があれば先に影を描く。`fill` は中身（枠の内側）を `row` の範囲 `[x0, x1)` に書く
/// （引数は x0 と、その範囲の行）。1 ピクセルの縁は `border` で、角は覆う割合で下と混ぜる
pub(super) fn panel_row(
    row: &mut [u32],
    y: i32,
    rect: (i32, i32, i32, i32),
    corner: &Corner,
    shadow: Option<(&Shadow, u8)>,
    border: u32,
    fill: impl FnOnce(i32, &mut [u32]),
) {
    let (px, py, pw, ph) = rect;
    let width = row.len() as i32;
    if let Some((s, a)) = shadow {
        let mut c = Canvas::new(row, width as usize, 1);
        c.shadow_round(px, py - y, pw, ph, theme::SHADOW_DY, s, corner, rgb(0, 0, 0), a);
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
    let mut below = [0u32; 2 * theme::RADIUS];
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
    // 縁
    if dy == 0 || dy == ph - 1 {
        row[x0 as usize..x1 as usize].fill(border);
    } else {
        for x in [px, px + pw - 1] {
            if x >= x0 && x < x1 {
                row[x as usize] = border;
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
        let inner = if dx >= 1 && dy >= 1 { corner.at(dx - 1, dy - 1, pw - 2, ph - 2) } else { 0 };
        let inside = row[x as usize];
        let c = blend(border, inside, inner);
        row[x as usize] = blend(below[i as usize], c, outer);
    }
}

/// マウスカーソルを `row`（画面の `y` 行目）に重ねる
pub(super) fn cursor_row(row: &mut [u32], y: i32, cx: i32, cy: i32, cursor: &[u32]) {
    let width = row.len();
    Canvas::new(row, width, 1).blit_alpha(cx, cy - y, cursor, CURSOR_W);
}

/// 1/8 ピクセル単位の多角形の中か（nonzero）
fn inside(poly: &[(i32, i32)], x: i32, y: i32) -> bool {
    let mut wind = 0;
    for k in 0..poly.len() {
        let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
        let cross = (b.0 - a.0) as i64 * (y - a.1) as i64 - (x - a.0) as i64 * (b.1 - a.1) as i64;
        if a.1 <= y {
            if b.1 > y && cross > 0 {
                wind += 1;
            }
        } else if b.1 <= y && cross < 0 {
            wind -= 1;
        }
    }
    wind != 0
}

/// 多角形が各ピクセルを覆う割合（0〜255）。1 ピクセルを 4×4 の点で数える
fn coverage(poly: &[(i32, i32)]) -> Vec<u8> {
    let mut out = vec![0u8; CURSOR_W * CURSOR_H];
    for (i, o) in out.iter_mut().enumerate() {
        let (px, py) = ((i % CURSOR_W) as i32 * 8, (i / CURSOR_W) as i32 * 8);
        let mut n = 0u32;
        for sy in 0..4 {
            for sx in 0..4 {
                if inside(poly, px + sx * 2 + 1, py + sy * 2 + 1) {
                    n += 1;
                }
            }
        }
        *o = (n * 255 / 16) as u8;
    }
    out
}

/// 矢印のカーソル: 白の地に黒い縁、右下に薄い影（整数だけで作る）
fn build_cursor() -> Vec<u32> {
    // 1/8 ピクセル単位。外側（黒い縁）と、1 ピクセルほど内側（白）
    const OUTER: [(i32, i32); 7] = [(0, 0), (0, 136), (36, 104), (60, 156), (80, 148), (56, 96), (100, 96)];
    const INNER: [(i32, i32); 7] = [(10, 24), (10, 112), (38, 88), (64, 142), (68, 140), (44, 86), (76, 86)];
    let outer = coverage(&OUTER);
    let inner = coverage(&INNER);
    // 影: 外側を (1, 2) ずらし、3×3 でぼかして 35% の濃さに
    let mut shade = vec![0u32; CURSOR_W * CURSOR_H];
    for y in 0..CURSOR_H as i32 {
        for x in 0..CURSOR_W as i32 {
            let mut sum = 0u32;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (sx, sy) = (x - 1 + dx, y - 2 + dy);
                    if sx >= 0 && sy >= 0 && (sx as usize) < CURSOR_W && (sy as usize) < CURSOR_H {
                        sum += outer[sy as usize * CURSOR_W + sx as usize] as u32;
                    }
                }
            }
            shade[y as usize * CURSOR_W + x as usize] = sum / 9 * 90 / 255;
        }
    }
    (0..CURSOR_W * CURSOR_H)
        .map(|i| {
            let (o, w) = (outer[i] as u32, inner[i] as u32);
            // 矢印の色: 縁（黒）と中（白）を内側の覆う割合で混ぜ、外側の覆う割合を濃さにする
            let color = blend(rgb(0, 0, 0), rgb(255, 255, 255), w as u8);
            let arrow = premultiply(color, o as u8);
            // 矢印の下に影（黒）を重ねる: 事前に掛けたアルファどうしの合成
            let sa = shade[i] * (255 - o) / 255;
            let a = (arrow >> 24) + sa;
            (a.min(255) << 24) | (arrow & 0x00FF_FFFF)
        })
        .collect()
}
