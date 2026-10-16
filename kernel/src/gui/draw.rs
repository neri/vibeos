//! デスクトップの見た目（VGA のモード 12h、640x480・16 色。docs/plans/vga16.md、D-072・D-073）
//!
//! Windows 3.x 風: 明るい灰色の地に白と濃い灰色の縁の立体。色は `palette::VGA16` の番号（0〜15）だけで、
//! 混ぜた色は作らない。文字は 8x16 の固定幅（VGA の BIOS のフォント。`vga::font16`）、太字は重ね打ち。
//!
//! - 上端のバー（20）: 明るい灰色の地（上に白・下に黒の線）。左に「VibeOS」（メニュー）、続けてウィンドウの
//!   ボタン（立体。フォーカスのあるものと開いているメニューのボタンはへこむ）、右に状態の表示
//! - ウィンドウ: 1 ピクセルの黒い枠、20 のタイトルバー（フォーカスがあれば紫に白い太字、無ければ濃い灰色に
//!   明るい灰色の字。下に黒い線）、右に [_]・[x] の立体のボタン。影も角の丸みも無い
//! - メニュー: 明るい灰色の地に黒い枠。選んだ項目は紫の地に白い字
//! - 背景: 青緑の単色か、16 色に誤差拡散した背景の画像
//! - タイトルバーをドラッグしている間は、ウィンドウの代わりに白黒の点線の枠だけを動かし、離したときに
//!   1 回だけ描き直す（486 と ISA の VGA では中身ごと動かすと引っかかるため。§6 の 6）
//!
//! 重ね合わせは色番号の 1 行（`compose_row`）で、`vga::write_row4_span` で 4 枚の面に分けて送る。
//! 背景の画像は読むときに 1 回だけ 16 色に誤差拡散してある（`Wallpaper::pixels`）。

use alloc::string::String;

use vibe_font::Font;
use vibe_gfx::Surface;
use vibe_gfx::palette::{BLACK, CYAN, DARK_GRAY, LIGHT_GRAY, MAGENTA, WHITE};

use super::bar;
use super::{Screen, WinId, Window};
use crate::drivers::vga;

/// タイトルバー（下の黒い線を含む）と上端のバーの高さ、メニューの 1 項目の高さ
pub const TITLE_H: usize = 20;
pub const BAR_H: usize = 20;
pub const MENU_ITEM_H: usize = 18;

/// 背景・タイトルバー（フォーカスの有無）の色
const DESKTOP: u8 = CYAN;
const TITLE_ACTIVE: u8 = MAGENTA;
const TITLE_INACTIVE: u8 = DARK_GRAY;
const FACE: u8 = LIGHT_GRAY;
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

/// 出っ張った立体の枠（外側は白と黒、内側は明るい灰色と濃い灰色）
pub(super) fn raised(s: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    s.bevel(x, y, w, h, WHITE, BLACK);
    s.bevel(x + 1, y + 1, w - 2, h - 2, FACE, DARK_GRAY);
}

/// へこんだ立体の枠
pub(super) fn sunken(s: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    s.bevel(x, y, w, h, DARK_GRAY, WHITE);
}

/// 16 色のアイコン（16x16）を (x, y) に置く。`dim` なら色のある画素を 1 つおきにして薄く見せる
fn draw_icon(s: &mut Surface, x: i32, y: i32, icon: &[u8], dim: bool) {
    let n = crate::gfx::icons::SIZE;
    for (i, &c) in icon.iter().enumerate().take(n * n) {
        let (ix, iy) = ((i % n) as i32, (i / n) as i32);
        if c != CLEAR && !(dim && (ix + iy) % 2 == 1) {
            s.put_pixel(x + ix, y + iy, c);
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
    let (bg, fg) = if focused { (TITLE_ACTIVE, WHITE) } else { (TITLE_INACTIVE, LIGHT_GRAY) };
    let band = h as i32 - 1;
    s.fill_rect(0, 0, ww as i32, band, bg);
    s.hline(0, band, ww as i32, BLACK);
    let f = font();
    let ty = (band - f.height as i32) / 2;
    let mut tx = 4;
    if let Some(icon) = w.icon {
        draw_icon(&mut s, 2, (band - 16) / 2, icon, false);
        tx = 22;
    }
    let title = take(&w.title, text_fit(&w.title, min_x - tx - 4));
    if focused {
        s.draw_text_bold(f, tx, ty, &title, fg);
    } else {
        s.draw_text(f, tx, ty, &title, fg, None);
    }
    // ボタン: 押せる所（幅 BUTTON_W）の中に 16x15 の立体
    let (bw, bh, by) = (16, 15, 2);
    let button = |s: &mut Surface, x: i32| -> (i32, i32) {
        let bx = x + (super::BUTTON_W as i32 - bw) / 2;
        s.fill_rect(bx, by, bw, bh, FACE);
        raised(s, bx, by, bw, bh);
        (bx, by)
    };
    let (bx, by2) = button(&mut s, min_x);
    s.fill_rect(bx + 5, by2 + 9, 6, 2, BLACK);
    if let Some(x) = close_x {
        let (bx, by2) = button(&mut s, x);
        for d in 0..6 {
            for (px, py) in [(bx + 4 + d, by2 + 4 + d), (bx + 9 - d, by2 + 4 + d)] {
                s.put_pixel(px, py, BLACK);
                s.put_pixel(px + 1, py, BLACK);
            }
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
        s.fill_rect(0, 0, w as i32, h, FACE);
        s.hline(0, 0, w as i32, WHITE);
        s.hline(0, h - 1, w as i32, BLACK);
        let ty = (h - f.height as i32) / 2;
        // 「VibeOS」（メニュー）
        let mw = bar::MENU_BUTTON_W - bar::BUTTON_GAP;
        if self.menu.is_some() {
            sunken(&mut s, 1, 1, mw - 1, h - 2);
        }
        s.draw_text_bold(f, 8, ty, "VibeOS", BLACK);
        for (id, x, bw) in buttons {
            let win = &self.windows[&id];
            let bw = bw - bar::BUTTON_GAP;
            if !win.minimized && Some(id) == focused {
                sunken(&mut s, x, 1, bw, h - 2);
            } else {
                raised(&mut s, x, 1, bw, h - 2);
            }
            let mut tx = x + 4;
            if let Some(icon) = win.icon {
                draw_icon(&mut s, x + 2, (h - 16) / 2, icon, win.minimized);
                tx = x + 20;
            }
            let title = take(&win.title, text_fit(&win.title, x + bw - 3 - tx));
            s.draw_text(f, tx, ty, &title, if win.minimized { DARK_GRAY } else { BLACK }, None);
        }
        let x = w as i32 - text_width(&self.status_text) - bar::STATUS_PAD;
        s.draw_text(f, x, ty, &self.status_text, BLACK, None);
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
        s.fill_rect(0, 0, w as i32, h as i32, FACE);
        s.rect(0, 0, w as i32, h as i32, BLACK);
        let ih = MENU_ITEM_H as i32;
        for (i, item) in m.items.iter().enumerate() {
            let x = (i / m.rows * m.col_w) as i32;
            let y = (bar::MENU_PAD + i % m.rows * MENU_ITEM_H) as i32;
            let fg = if m.hover == Some(i) {
                s.fill_rect(x + 2, y, m.col_w as i32 - 4, ih, TITLE_ACTIVE);
                WHITE
            } else {
                BLACK
            };
            let app = if item == super::MENU_TERMINAL { "terminal" } else { item.as_str() };
            if let Some(icon) = crate::gfx::icons::for_app(app) {
                draw_icon(&mut s, x + 4, y + (ih - 16) / 2, icon, false);
            }
            s.draw_text(f, x + 24, y + (ih - f.height as i32) / 2, item, fg, None);
        }
        let bottom = bar::menu_y() + h;
        self.mark_rows(BAR_H, bottom);
    }

    /// 背景の `y` 行目（色番号）
    fn background_row(&self, y: usize, row: &mut [u8]) {
        let wp = self.wallpaper.as_deref();
        if !wp.is_some_and(|wp| wp.fit == vibe_gfx::wallpaper::Fit::Tile) {
            row.fill(DESKTOP);
        }
        if let Some(wp) = wp {
            vibe_gfx::wallpaper::fill_row(row, y, &wp.pixels, wp.w, wp.h, self.height, wp.fit);
        }
    }

    /// デスクトップの 1 行（色番号）: バー → 背景 → ウィンドウ（奥から）→ メニュー → ドラッグの枠 → カーソル
    pub(super) fn compose_row(&self, y: usize, row: &mut [u8], _focused: Option<WinId>) {
        if y < BAR_H {
            if self.bar.len() == self.width * BAR_H {
                row.copy_from_slice(&self.bar[y * self.width..(y + 1) * self.width]);
            }
        } else {
            self.background_row(y, row);
            for &id in &self.order {
                self.draw_window_row(id, y as i32, row);
            }
            self.draw_menu_row(y, row);
            self.draw_outline_row(y as i32, row);
        }
        if self.cursor_visible {
            self.draw_cursor_row(y, row);
        }
    }

    /// ウィンドウ（黒い枠・タイトルバー・中身）の `y` 行目を重ねる
    fn draw_window_row(&self, id: WinId, y: i32, row: &mut [u8]) {
        let w = &self.windows[&id];
        if w.minimized {
            return;
        }
        let (top, bottom) = (w.y - TITLE_H as i32 - 1, w.y + w.h as i32);
        if y < top || y > bottom {
            return;
        }
        let sw = row.len() as i32;
        let (left, right) = (w.x - 1, w.x + w.w as i32);
        let mut put = |x: i32, c: u8| {
            if (0..sw).contains(&x) {
                row[x as usize] = c;
            }
        };
        if y == top || y == bottom {
            for x in left.max(0)..=right.min(sw - 1) {
                put(x, BLACK);
            }
            return;
        }
        put(left, BLACK);
        put(right, BLACK);
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

    fn draw_menu_row(&self, y: usize, row: &mut [u8]) {
        let Some(m) = &self.menu else { return };
        let my = bar::menu_y();
        if y < my || y >= my + m.h || m.buf.len() != m.w * m.h {
            return;
        }
        let x0 = bar::MENU_X as usize;
        let n = m.w.min(row.len().saturating_sub(x0));
        let r = y - my;
        row[x0..x0 + n].copy_from_slice(&m.buf[r * m.w..r * m.w + n]);
    }

    /// ドラッグしている枠（ウィンドウ全体の外側。2 ピクセルの白黒の点線）の矩形 (x, y, w, h)
    fn outline_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let (id, ..) = self.drag?;
        let (ox, oy) = self.outline?;
        let w = self.windows.get(&id)?;
        Some((ox - 1, oy - TITLE_H as i32 - 1, w.w as i32 + 2, w.h as i32 + TITLE_H as i32 + 2))
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
        let (sw, sh) = (self.width as i32, self.height as i32);
        let Some((id, ..)) = self.drag else { return };
        let Some(w) = self.windows.get(&id) else { return };
        let pos = (x.clamp(-(w.w as i32) + 40, sw - 40), y.clamp((BAR_H + TITLE_H + 1) as i32, sh - 4));
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
