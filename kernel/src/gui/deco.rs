//! デスクトップの飾り: 角の丸いウィンドウ・パネル（メニュー）の影、アルファ付きのマウスカーソル
//!
//! パネルは縁を描かず、2 層の影（近い影と遠い大きな影）で背景と分ける。1 行ずつ重ねるのは
//! `vibe_gfx::panel`（ホストでテストする）。角の丸みと影の表（`Corner`・`Shadow`）は
//! デスクトップにするときに 1 回だけ作る。

use alloc::vec;
use alloc::vec::Vec;

use vibe_gfx::canvas::{Canvas, Corner, Shadow, blend, premultiply, rgb};
use vibe_gfx::panel::{self, PanelStyle, ShadowLayer};
use vibe_gfx::theme;

/// マウスカーソルの画像の大きさ（矢印の先が左上 (0, 0)）
pub(super) const CURSOR_W: usize = 16;
pub(super) const CURSOR_H: usize = 23;

pub(super) struct Deco {
    pub(super) corner: Corner,
    pub(super) corner_small: Corner,
    /// タイトルバーのボタンの丸（直径 BUTTON_W）
    pub(super) button: Corner,
    /// ドックのパネル・タイル・開いているアプリの点
    pub(super) dock: Corner,
    pub(super) tile: Corner,
    pub(super) dot: Corner,
    /// 影の近い層と遠い層
    pub(super) near: Shadow,
    pub(super) far: Shadow,
    /// 事前に掛けたアルファの 0xAARRGGBB（CURSOR_W × CURSOR_H）
    pub(super) cursor: Vec<u32>,
}

impl Deco {
    pub(super) fn new() -> Deco {
        Deco {
            corner: Corner::new(theme::RADIUS),
            corner_small: Corner::new(theme::RADIUS_SMALL),
            button: Corner::new(super::BUTTON_W / 2),
            dock: Corner::new(vibe_gfx::dock::RADIUS),
            tile: Corner::new(vibe_gfx::dock::TILE_RADIUS),
            dot: Corner::new(2),
            near: Shadow::new(theme::SHADOW_NEAR_BLUR),
            far: Shadow::new(theme::SHADOW_FAR_BLUR),
            cursor: build_cursor(),
        }
    }

    /// 2 層の影（遠い → 近い）。`focused` ならフォーカスのあるウィンドウ・メニューの濃さ
    pub(super) fn shadows(&self, focused: bool) -> [ShadowLayer<'_>; 2] {
        let ((near, far), far_dy) = if focused {
            (theme::SHADOW_ALPHA, theme::SHADOW_FAR_DY)
        } else {
            (theme::SHADOW_ALPHA_INACTIVE, theme::SHADOW_FAR_DY_INACTIVE)
        };
        [
            ShadowLayer { shadow: &self.far, dy: far_dy, alpha: far },
            ShadowLayer { shadow: &self.near, dy: theme::SHADOW_NEAR_DY, alpha: near },
        ]
    }
}

/// 影がパネルの外へはみ出す幅（描き直す行を広げるのに使う）。遠い影の「ぼかし + 下へ」
pub(super) const SHADOW_MARGIN: i32 = theme::SHADOW_FAR_BLUR as i32 + theme::SHADOW_FAR_DY;

/// 縁の無い角の丸いパネル（ウィンドウ・メニュー）の `y` 行目を `row` に重ねる（`vibe_gfx::panel`）
pub(super) fn panel_row(
    row: &mut [u32],
    y: i32,
    rect: (i32, i32, i32, i32),
    corner: &Corner,
    shadows: &[ShadowLayer],
    fill: impl FnOnce(i32, &mut [u32]),
) {
    let style = PanelStyle { corner, shadows, shadow_color: theme::SHADOW_COLOR, border: None };
    panel::panel_row(row, y, rect, &style, fill);
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
