//! デスクトップの上端のバーとメニュー
//!
//! バー: 左に「VibeOS」（メニューのボタン）、続けて開いているウィンドウのボタン（作った順）、
//! 右に状態の表示（ステータスバーのスレッドが `set_status_text` で渡す文字列）。
//! ウィンドウのボタンはアイコンと題名で、フォーカスのあるものは薄い強調色の角の丸い地、最小化したものは薄い文字。
//! 32 ビットで `Screen::bar` に描く（文字は UI のフォント。状態の表示は数が変わっても位置が動かないよう固定幅）。
//!
//! メニュー: 「VibeOS」を押すとバーの下に開く。項目は "Terminal" と /BIN のアプリ（開くたびに読む）。
//! 角の丸いパネルに影を付けて出し、項目はアイコンと名前。乗せた項目は強調色の地。
//! 画面の高さに入りきらなければ、何列かに分けて並べる（上から下、左から右の順）。
//! 項目を押すと起動する（ロックの外で。`super::launch`）。どこを押してもメニューは閉じる。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_gfx::canvas::{Canvas, rgb};
use vibe_gfx::theme;

use super::deco::{self, SHADOW_MARGIN};
use super::{BAR_H, Face, Screen, WinId, draw_label, label_fit, label_width};

/// 「VibeOS」のボタンの幅（文字は x = 10 から）
const MENU_BUTTON_W: i32 = 72;
const BUTTON_MAX_W: i32 = 140;
const BUTTON_MIN_W: i32 = 32;
const BUTTON_GAP: i32 = 2;
/// 状態の表示の右の余白
const STATUS_PAD: i32 = 8;
/// メニューのパネルの左上（バーの下）、1 項目の高さと上下の余白
const MENU_X: i32 = 2;
const MENU_Y: usize = BAR_H + 2;
const ITEM_H: usize = 22;
const MENU_PAD: usize = 2;
/// メニューの項目のアイコンと名前の位置（列の左端から）
const ITEM_ICON_X: i32 = 10;
const ITEM_TEXT_X: i32 = 34;
const MENU_SHADOW_ALPHA: u8 = 90;

pub(super) struct Menu {
    pub(super) items: Vec<String>,
    hover: Option<usize>,
    w: usize,
    h: usize,
    /// 1 列の項目の数と列の幅
    rows: usize,
    col_w: usize,
    buf: Vec<u32>,
}

/// バーのどこを押したか
pub(super) enum BarHit {
    Menu,
    Window(WinId),
}

impl Screen {
    /// 状態の表示（右寄せ）を変える
    pub fn set_status_text(&mut self, text: &str) {
        if self.status_text != text {
            self.status_text.clear();
            self.status_text.push_str(text);
            self.bar_dirty = true;
        }
    }

    /// ウィンドウのボタンの並び: (ウィンドウ, 左端, 幅)。作った順（番号の順）
    fn bar_buttons(&self) -> Vec<(WinId, i32, i32)> {
        let right = self.width as i32 - label_width(Face::Mono, &self.status_text) - 2 * STATUS_PAD;
        let n = self.windows.len() as i32;
        if n == 0 {
            return Vec::new();
        }
        let room = right - MENU_BUTTON_W;
        let w = (room / n).clamp(BUTTON_MIN_W, BUTTON_MAX_W);
        self.windows
            .keys()
            .enumerate()
            .map(|(i, &id)| (id, MENU_BUTTON_W + i as i32 * w, w))
            .take_while(|&(_, x, w)| x + w <= right.max(MENU_BUTTON_W + w))
            .collect()
    }

    pub(super) fn bar_hit(&self, x: i32) -> Option<BarHit> {
        if x < MENU_BUTTON_W {
            return Some(BarHit::Menu);
        }
        self.bar_buttons()
            .into_iter()
            .find(|&(_, bx, bw)| x >= bx && x < bx + bw - BUTTON_GAP)
            .map(|(id, _, _)| BarHit::Window(id))
    }

    /// バーを `self.bar` に描き直す
    pub(super) fn render_bar(&mut self) {
        self.bar_dirty = false;
        let w = self.width;
        if self.bar.len() != w * BAR_H {
            return;
        }
        let Some(deco) = &self.deco else { return };
        let focused = self.focused();
        let buttons = self.bar_buttons();
        let h = BAR_H as i32;
        let mut bar = core::mem::take(&mut self.bar);
        let mut c = Canvas::new(&mut bar, w, BAR_H);
        c.clear(theme::BAR);
        c.hline(0, h - 1, w as i32, theme::SEPARATOR);
        let pill = &deco.corner_small;
        if self.menu.is_some() {
            c.fill_round_rect(2, 3, MENU_BUTTON_W - BUTTON_GAP - 2, h - 7, pill, theme::HOVER, 255);
        }
        draw_label(&mut c, Face::Bold, 10, 0, h - 1, "VibeOS", theme::ACCENT);
        for (id, x, bw) in buttons {
            let win = &self.windows[&id];
            let (bg, fg) = if win.minimized {
                (None, theme::TEXT_DIM)
            } else if Some(id) == focused {
                (Some(theme::ACCENT_LIGHT), theme::TEXT)
            } else {
                (None, theme::TEXT)
            };
            if let Some(bg) = bg {
                c.fill_round_rect(x, 3, bw - BUTTON_GAP, h - 7, pill, bg, 255);
            }
            let mut tx = x + 6;
            if let Some(icon) = win.icon {
                let s = crate::gfx::icons::SIZE as i32;
                c.blit_alpha(x + 5, (h - 1 - s) / 2, icon, s as usize);
                tx = x + 5 + s + 5;
            }
            let n = label_fit(Face::Ui, &win.title, x + bw - BUTTON_GAP - 6 - tx);
            let title: String = win.title.chars().take(n).collect();
            draw_label(&mut c, Face::Ui, tx, 0, h - 1, &title, fg);
        }
        let x = w as i32 - label_width(Face::Mono, &self.status_text) - STATUS_PAD;
        draw_label(&mut c, Face::Mono, x, 0, h - 1, &self.status_text, theme::TEXT);
        self.bar = bar;
        self.mark_rows(0, BAR_H);
    }

    pub(super) fn open_menu(&mut self, items: Vec<String>) {
        let longest = items.iter().map(|i| label_width(Face::Ui, i)).max().unwrap_or(0);
        let col_w = (longest as usize + ITEM_TEXT_X as usize + 20).max(130);
        let room = self.height - MENU_Y - 2 * MENU_PAD - SHADOW_MARGIN as usize;
        let rows = (room / ITEM_H).clamp(1, items.len().max(1));
        let cols = items.len().div_ceil(rows).max(1);
        let w = (cols * col_w).min(self.width);
        let h = rows * ITEM_H + 2 * MENU_PAD;
        self.menu = Some(Menu { items, hover: None, w, h, rows, col_w, buf: vec![0; w * h] });
        self.render_menu();
        self.bar_dirty = true;
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.mark_rows(BAR_H, MENU_Y + m.h + SHADOW_MARGIN as usize);
            self.bar_dirty = true;
        }
    }

    fn render_menu(&mut self) {
        let Some(deco) = &self.deco else { return };
        let Some(m) = &mut self.menu else { return };
        let (w, h) = (m.w, m.h);
        let mut c = Canvas::new(&mut m.buf, w, h);
        c.clear(theme::FACE);
        for (i, item) in m.items.iter().enumerate() {
            let x = (i / m.rows * m.col_w) as i32;
            let y = (MENU_PAD + i % m.rows * ITEM_H) as i32;
            let fg = if m.hover == Some(i) {
                c.fill_round_rect(x + 4, y + 1, m.col_w as i32 - 8, ITEM_H as i32 - 2, &deco.corner_small, theme::ACCENT, 255);
                rgb(255, 255, 255)
            } else {
                theme::TEXT
            };
            let app = if item == super::MENU_TERMINAL { "terminal" } else { item.as_str() };
            if let Some(icon) = crate::gfx::icons::for_app(app) {
                let s = crate::gfx::icons::SIZE as i32;
                c.blit_alpha(x + ITEM_ICON_X, y + (ITEM_H as i32 - s) / 2, icon, s as usize);
            }
            draw_label(&mut c, Face::Ui, x + ITEM_TEXT_X, y, ITEM_H as i32, item, fg);
        }
        self.mark_rows(BAR_H, MENU_Y + h + SHADOW_MARGIN as usize);
    }

    /// (x, y) にあるメニューの項目。メニューの外なら None、中でも項目でなければ Some(None)
    pub(super) fn menu_item_at(&self, x: i32, y: i32) -> Option<Option<usize>> {
        let m = self.menu.as_ref()?;
        let (mx, my) = (x - MENU_X, y - MENU_Y as i32);
        if mx < 0 || my < 0 || mx >= m.w as i32 || my >= m.h as i32 {
            return None;
        }
        let row = (my - MENU_PAD as i32).div_euclid(ITEM_H as i32);
        if my < MENU_PAD as i32 || row as usize >= m.rows {
            return Some(None);
        }
        let i = mx as usize / m.col_w * m.rows + row as usize;
        Some((i < m.items.len()).then_some(i))
    }

    /// マウスの下の項目を目立たせる
    pub(super) fn menu_hover(&mut self, x: i32, y: i32) {
        let hover = self.menu_item_at(x, y).flatten();
        if let Some(m) = &mut self.menu {
            if m.hover != hover {
                m.hover = hover;
                self.render_menu();
            }
        }
    }

    /// 重ね合わせ: メニュー（角の丸いパネルと影）の行を row に重ねる
    pub(super) fn draw_menu_row(&self, y: usize, row: &mut [u32]) {
        let (Some(m), Some(deco)) = (&self.menu, &self.deco) else { return };
        let rect = (MENU_X, MENU_Y as i32, m.w as i32, m.h as i32);
        let fill = |x0: i32, span: &mut [u32]| {
            let r = y - MENU_Y;
            let s0 = (x0 - MENU_X) as usize;
            let n = span.len().min(m.w - s0);
            span[..n].copy_from_slice(&m.buf[r * m.w + s0..r * m.w + s0 + n]);
        };
        deco::panel_row(row, y as i32, rect, &deco.corner, Some((&deco.shadow, MENU_SHADOW_ALPHA)), theme::BORDER, fill);
    }
}
