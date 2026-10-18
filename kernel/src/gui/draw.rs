//! デスクトップの見た目（VGA のモード 12h の白黒。docs/plans/mono.md、D-077）
//!
//! 白と黒だけで描く「今風の 1 ビット」: 平らな面と太い線、右下にずらした黒い影。色番号は黒（0）と白（15）
//! だけを使い（画面に出すときの白黒の模様は `vibe_gfx::mono`）、灰色は濃い灰（8 = 50% の市松）だけ。
//! 文字は 8x16 の固定幅（VGA の BIOS のフォント。`vga::font16`）、太字は重ね打ち。
//!
//! - 上端のバー（20）: 黒い地に白い字。左に「VibeOS」（メニュー。開いている間は白い角丸に黒い字）、続けて
//!   ウィンドウのボタン（白黒を入れ替えたアイコンと名前。フォーカスのあるものは太字と 2 ピクセルの下線、
//!   最小化したものは灰色の字と薄いアイコン）、右に状態の表示
//! - ウィンドウ: 2 ピクセルの黒い枠、20 のタイトルバー（中 18 と下の黒い線 2。フォーカスがあれば黒い地に白い
//!   太字、無ければ白い地に黒い太字）、右に [_]・[x] の枠だけの箱。右下に黒い影（フォーカスのあるもの 4、
//!   ほかは 2 ピクセル）
//! - メニュー: 白い地に 2 ピクセルの黒い枠と 4 ピクセルの影。選んだ項目は黒い角丸に白い字
//! - 背景: 白い地に斜めの細い線（6 ピクセルおき）か、背景の画像
//! - タイトルバーをドラッグしている間は、ウィンドウの代わりに白黒の点線の枠だけを動かし、離したときに
//!   1 回だけ描き直す（486 と ISA の VGA では中身ごと動かすと引っかかるため）
//!
//! 重ね合わせは色番号の 1 行（`compose_row`）で、`vga::write_row_mono` が白黒にして送る。

use alloc::string::String;

use vibe_font::Font;
use vibe_gfx::Surface;
use vibe_gfx::palette::{BLACK, DARK_GRAY, WHITE};

use super::bar;
use super::{Screen, WinId, Window};
use crate::drivers::vga;

/// タイトルバー（下の黒い線を含む）と上端のバーの高さ、メニューの 1 項目の高さ
pub const TITLE_H: usize = 20;
pub const BAR_H: usize = 20;
pub const MENU_ITEM_H: usize = 18;
/// ウィンドウの枠の太さ
pub const BORDER: i32 = 2;
/// タイトルバーの下の黒い線の太さ
const TITLE_LINE: i32 = 2;
/// ウィンドウの影（右下にずらす量）: フォーカスのあるもの・ほか。メニューの影
pub const SHADOW_FOCUSED: i32 = 4;
pub const SHADOW: i32 = 2;
pub const MENU_SHADOW: i32 = 4;
/// 背景の斜めの線の間隔
const DESKTOP_STRIPE: usize = 6;
/// 16 色のアイコンの透明な画素
const CLEAR: u8 = vibe_gfx::icons::TRANSPARENT;

pub(super) fn font() -> &'static Font {
    vga::font16()
}

/// 文字の幅（固定幅）
pub(super) fn text_width(text: &str) -> i32 {
    (text.chars().count() * font().width) as i32
}

/// 幅 `max` に入る先頭の文字数
pub(super) fn text_fit(text: &str, max: i32) -> usize {
    (max.max(0) as usize / font().width).min(text.chars().count())
}

/// 先頭の `n` 文字
fn take(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// 16x16 のアイコンを (x, y) に置く。`invert` なら白と黒を入れ替える（黒い地に出すとき）。
/// `dim` なら色のある画素を 1 つおきにして薄く見せる
fn draw_icon(s: &mut Surface, x: i32, y: i32, icon: &[u8], invert: bool, dim: bool) {
    let n = crate::gfx::icons::SIZE;
    for (i, &c) in icon.iter().enumerate().take(n * n) {
        let (ix, iy) = ((i % n) as i32, (i / n) as i32);
        if c != CLEAR && !(dim && (ix + iy) % 2 == 1) {
            s.put_pixel(x + ix, y + iy, if invert { vibe_gfx::mono::invert(c) } else { c });
        }
    }
}

/// タイトルバーを `w.titlebar` に描く
pub(super) fn render_titlebar(w: &mut Window, focused: bool) {
    let (ww, h) = (w.w, TITLE_H);
    w.titlebar.resize(ww * h, 0);
    let close_x = w.close_box_x();
    let min_x = w.minimize_box_x();
    let mut s = Surface::new(&mut w.titlebar, ww, h);
    let (bg, fg) = if focused { (BLACK, WHITE) } else { (WHITE, BLACK) };
    let band = h as i32 - TITLE_LINE;
    s.fill_rect(0, 0, ww as i32, band, bg);
    s.fill_rect(0, band, ww as i32, TITLE_LINE, BLACK);
    let f = font();
    let ty = (band - f.height as i32) / 2;
    let mut tx = 6;
    if let Some(icon) = w.icon {
        draw_icon(&mut s, 4, (band - 16) / 2, icon, focused, false);
        tx = 24;
    }
    let title = take(&w.title, text_fit(&w.title, min_x - tx - 4));
    s.draw_text_bold(f, tx, ty, &title, fg);
    // ボタン: 押せる所（幅 BUTTON_W）の中に 14x14 の枠だけの箱
    let bs = 14;
    let by = (band - bs) / 2;
    let button = |s: &mut Surface, x: i32| -> i32 {
        let bx = x + (super::BUTTON_W as i32 - bs) / 2;
        s.rect(bx, by, bs, bs, fg);
        bx
    };
    let bx = button(&mut s, min_x);
    s.fill_rect(bx + 4, by + 9, 6, 2, fg);
    if let Some(x) = close_x {
        let bx = button(&mut s, x);
        for d in 0..6 {
            s.put_pixel(bx + 4 + d, by + 4 + d, fg);
            s.put_pixel(bx + 9 - d, by + 4 + d, fg);
        }
    }
}

impl Screen {
    /// 上端のバーを `self.bar` に描き直す
    pub(super) fn render_bar(&mut self) {
        self.bar_dirty = false;
        let w = self.width;
        self.bar.resize(w * BAR_H, 0);
        let focused = self.focused();
        let buttons = self.bar_buttons();
        let mut buf = core::mem::take(&mut self.bar);
        let mut s = Surface::new(&mut buf, w, BAR_H);
        let f = font();
        let h = BAR_H as i32;
        s.fill_rect(0, 0, w as i32, h, BLACK);
        let ty = (h - f.height as i32) / 2;
        // 「VibeOS」（メニュー）。開いている間は白い角丸に黒い字
        let mw = bar::MENU_BUTTON_W - bar::BUTTON_GAP;
        let menu_fg = if self.menu.is_some() {
            s.fill_round_rect(2, 2, mw - 2, h - 4, 6, WHITE);
            BLACK
        } else {
            WHITE
        };
        s.draw_text_bold(f, 10, ty, "VibeOS", menu_fg);
        for (id, x, bw) in buttons {
            let win = &self.windows[&id];
            let bw = bw - bar::BUTTON_GAP;
            let mut tx = x + 4;
            if let Some(icon) = win.icon {
                draw_icon(&mut s, x + 2, (h - 16) / 2, icon, true, win.minimized);
                tx = x + 20;
            }
            let title = take(&win.title, text_fit(&win.title, x + bw - 3 - tx));
            if win.minimized {
                s.draw_text(f, tx, ty, &title, DARK_GRAY, None);
            } else if Some(id) == focused {
                s.draw_text_bold(f, tx, ty, &title, WHITE);
                s.fill_rect(tx, h - 3, text_width(&title) + 1, 2, WHITE);
            } else {
                s.draw_text(f, tx, ty, &title, WHITE, None);
            }
        }
        let x = w as i32 - text_width(&self.status_text) - bar::STATUS_PAD;
        s.draw_text(f, x, ty, &self.status_text, WHITE, None);
        self.bar = buf;
        self.mark_rows(0, BAR_H);
    }

    /// メニューの中身を描き直す
    pub(super) fn render_menu(&mut self) {
        let Some(m) = &mut self.menu else { return };
        let (w, h) = (m.w, m.h);
        m.buf.resize(w * h, 0);
        let mut s = Surface::new(&mut m.buf, w, h);
        let f = font();
        s.fill_rect(0, 0, w as i32, h as i32, WHITE);
        s.round_rect(0, 0, w as i32, h as i32, 0, BORDER, BLACK);
        let ih = MENU_ITEM_H as i32;
        for (i, item) in m.items.iter().enumerate() {
            let x = (i / m.rows * m.col_w) as i32;
            let y = (bar::MENU_PAD + i % m.rows * MENU_ITEM_H) as i32;
            let hover = m.hover == Some(i);
            let fg = if hover {
                s.fill_round_rect(x + 4, y, m.col_w as i32 - 8, ih, 4, BLACK);
                WHITE
            } else {
                BLACK
            };
            let app = if item == super::MENU_TERMINAL { "terminal" } else { item.as_str() };
            if let Some(icon) = crate::gfx::icons::for_app(app) {
                draw_icon(&mut s, x + 8, y + (ih - 16) / 2, icon, hover, false);
            }
            s.draw_text(f, x + 28, y + (ih - f.height as i32) / 2, item, fg, None);
        }
        let bottom = bar::menu_y() + h + MENU_SHADOW as usize;
        self.mark_rows(BAR_H, bottom);
    }

    /// 背景の `y` 行目（色番号）
    fn background_row(&self, y: usize, row: &mut [u8]) {
        let wp = self.wallpaper.as_deref();
        if !wp.is_some_and(|wp| wp.fit == vibe_gfx::wallpaper::Fit::Tile) {
            // 斜めの細い線: x + y が DESKTOP_STRIPE の倍数の所だけ黒
            row.fill(WHITE);
            let first = (DESKTOP_STRIPE - y % DESKTOP_STRIPE) % DESKTOP_STRIPE;
            for x in (first..row.len()).step_by(DESKTOP_STRIPE) {
                row[x] = BLACK;
            }
        }
        if let Some(wp) = wp {
            vibe_gfx::wallpaper::fill_row(row, y, &wp.pixels, wp.w, wp.h, self.height, wp.fit);
        }
    }

    /// デスクトップの 1 行（色番号）: バー → 背景 → ウィンドウ（奥から）→ メニュー → ドラッグの枠 → カーソル
    pub(super) fn compose_row(&self, y: usize, row: &mut [u8], focused: Option<WinId>) {
        if y < BAR_H {
            if self.bar.len() == self.width * BAR_H {
                row.copy_from_slice(&self.bar[y * self.width..(y + 1) * self.width]);
            }
        } else {
            self.background_row(y, row);
            for &id in &self.order {
                self.draw_window_row(id, y as i32, row, Some(id) == focused);
            }
            self.draw_menu_row(y, row);
            self.draw_outline_row(y as i32, row);
        }
        if self.cursor_visible {
            self.draw_cursor_row(y, row);
        }
    }

    /// ウィンドウ（黒い枠・タイトルバー・中身・影）の `y` 行目を重ねる
    fn draw_window_row(&self, id: WinId, y: i32, row: &mut [u8], focused: bool) {
        let w = &self.windows[&id];
        if w.minimized {
            return;
        }
        let (left, right, top, bottom) = w.frame();
        let shadow = if focused { SHADOW_FOCUSED } else { SHADOW };
        if y < top || y > bottom + shadow {
            return;
        }
        let sw = row.len() as i32;
        let mut fill = |x0: i32, x1: i32| {
            // [x0, x1] を黒く塗る（画面の外は切る）
            for x in x0.max(0)..=x1.min(sw - 1) {
                row[x as usize] = BLACK;
            }
        };
        // 影: 右（上から shadow 下がった所から）と下（左から shadow 右へずれた所から）
        if y > bottom {
            fill(left + shadow, right + shadow);
            return;
        }
        if y >= top + shadow {
            fill(right + 1, right + shadow);
        }
        if y < top + BORDER || y > bottom - BORDER {
            fill(left, right);
            return;
        }
        fill(left, left + BORDER - 1);
        fill(right - BORDER + 1, right);
        let (s0, s1) = ((-w.x).max(0) as usize, ((sw - w.x).min(w.w as i32)).max(0) as usize);
        if s0 >= s1 {
            return;
        }
        let d = &mut row[(w.x + s0 as i32) as usize..(w.x + s1 as i32) as usize];
        if y < w.y {
            let r = (y - (w.y - TITLE_H as i32)) as usize;
            if w.titlebar.len() == w.w * TITLE_H {
                d.copy_from_slice(&w.titlebar[r * w.w + s0..r * w.w + s1]);
            }
            return;
        }
        d.copy_from_slice(&self.client_row(id, (y - w.y) as usize)[s0..s1]);
    }

    /// メニュー（と右下の影）の `y` 行目を重ねる
    fn draw_menu_row(&self, y: usize, row: &mut [u8]) {
        let Some(m) = &self.menu else { return };
        let my = bar::menu_y();
        if y < my || y >= my + m.h + MENU_SHADOW as usize || m.buf.len() != m.w * m.h {
            return;
        }
        let x0 = bar::MENU_X as usize;
        let sh = MENU_SHADOW as usize;
        let black = |row: &mut [u8], a: usize, b: usize| {
            let (a, b) = (a.min(row.len()), b.min(row.len()));
            row[a..b].fill(BLACK);
        };
        if y >= my + m.h {
            black(row, x0 + sh, x0 + m.w + sh);
            return;
        }
        let n = m.w.min(row.len().saturating_sub(x0));
        let r = y - my;
        row[x0..x0 + n].copy_from_slice(&m.buf[r * m.w..r * m.w + n]);
        if y >= my + sh {
            black(row, x0 + m.w, x0 + m.w + sh);
        }
    }

    /// ドラッグしている枠（ウィンドウの枠の外側。2 ピクセルの白黒の点線）の矩形 (x, y, w, h)
    fn outline_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let (id, ..) = self.drag?;
        let (ox, oy) = self.outline?;
        let w = self.windows.get(&id)?;
        let b = BORDER;
        Some((ox - b, oy - TITLE_H as i32 - b, w.w as i32 + 2 * b, w.h as i32 + TITLE_H as i32 + 2 * b))
    }

    fn draw_outline_row(&self, y: i32, row: &mut [u8]) {
        let Some((x, top, w, h)) = self.outline_rect() else { return };
        if y < top || y >= top + h {
            return;
        }
        let sw = row.len() as i32;
        let dot = |row: &mut [u8], px: i32| {
            if (0..sw).contains(&px) {
                row[px as usize] = if (px + y) % 2 == 0 { BLACK } else { WHITE };
            }
        };
        if y < top + 2 || y >= top + h - 2 {
            for px in x..x + w {
                dot(row, px);
            }
        } else {
            for px in [x, x + 1, x + w - 2, x + w - 1] {
                dot(row, px);
            }
        }
    }

    /// 枠を (x, y)（中身の左上）へ動かす（ドラッグ中）。前と後の枠の行を描き直す
    pub(super) fn move_outline(&mut self, x: i32, y: i32) {
        let Some((id, ..)) = self.drag else { return };
        let Some(w) = self.windows.get(&id) else { return };
        let pos = self.clamp_position(w, x, y);
        if self.outline == Some(pos) {
            return;
        }
        self.mark_outline();
        self.outline = Some(pos);
        self.mark_outline();
    }

    fn mark_outline(&mut self) {
        if let Some((x, y, w, h)) = self.outline_rect() {
            self.mark_rect(y.max(0) as usize, (y + h).max(0) as usize, x.max(0) as usize, (x + w).max(0) as usize);
        }
    }

    /// ドラッグを終える: ウィンドウを枠の位置へ動かす
    pub(super) fn finish_outline(&mut self) {
        let Some((id, ..)) = self.drag else { return };
        self.mark_outline();
        if let Some((x, y)) = self.outline.take() {
            self.move_window(id, x, y);
        }
    }
}
