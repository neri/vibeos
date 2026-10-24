//! デスクトップの上端のバーとメニュー
//!
//! バー: 左に「VibeOS」（メニューのボタン）、続けて開いているウィンドウのボタン（作った順）、
//! 右に状態の表示（ステータスバーのスレッドが `set_status_text` で渡す文字列）。
//! 地は塗らず、背景（既定の背景か背景の画像）の上に白い文字とアイコンを描く。背景の画像のときだけ、
//! 明るい画像でも読めるようバーの範囲に黒を重ねる。ウィンドウのボタンはアイコンと題名で、フォーカスのあるものと
//! 開いているメニューのボタンは白を薄く重ねた角の丸い地、最小化したものは薄い文字と薄いアイコン。
//! 32 ビットで `Screen::bar` に描く（文字は UI のフォント。状態の表示は数が変わっても位置が動かないよう固定幅）。
//! 背景が変わったら描き直す（`set_wallpaper` が `bar_dirty` にする）。
//!
//! メニュー: 「VibeOS」を押すとバーの下に開く。項目は "Terminal" と /BIN のアプリ（開くたびに読む）。
//! 縁の無い角の丸いパネルに 2 層の影を付けて出し、項目はアイコンと名前。乗せた項目は薄い強調色の地に濃い強調色の文字。
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

pub(super) struct Menu {
    pub(super) items: Vec<String>,
    hover: Option<usize>,
    w: usize,
    h: usize,
    /// 1 列の項目の数と列の幅
    rows: usize,
    col_w: usize,
    buf: Vec<u32>,
    /// すりガラス: 後ろをぼかして色を重ねたもの（w × h。blur=off なら空）と、後回しにした・計算した刻み
    pub(super) glass: Vec<u32>,
    pub(super) glass_deferred: bool,
    pub(super) glass_at: u32,
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
        // 地: 背景の行そのもの（背景の画像なら黒を重ねる）
        for (y, row) in bar.chunks_exact_mut(w).enumerate() {
            self.background_row(y, row);
        }
        let mut c = Canvas::new(&mut bar, w, BAR_H);
        if self.wallpaper.is_some() {
            c.fill_rect_alpha(0, 0, w as i32, h, rgb(0, 0, 0), theme::BAR_SHADE_ALPHA);
        }
        let pill = &deco.corner_small;
        let white = rgb(255, 255, 255);
        if self.menu.is_some() {
            c.fill_round_rect(2, 3, MENU_BUTTON_W - BUTTON_GAP - 2, h - 6, pill, white, theme::BAR_PILL_ALPHA);
        }
        draw_label(&mut c, Face::Bold, 10, 0, h, "VibeOS", theme::BAR_TEXT);
        for (id, x, bw) in buttons {
            let win = &self.windows[&id];
            if !win.minimized && Some(id) == focused {
                c.fill_round_rect(x, 3, bw - BUTTON_GAP, h - 6, pill, white, theme::BAR_PILL_ALPHA);
            }
            let (fg, fade) = if win.minimized { (theme::BAR_TEXT_DIM, 128) } else { (theme::BAR_TEXT, 255) };
            let mut tx = x + 6;
            if let Some(icon) = win.icon {
                let s = crate::gfx::icons::SIZE as i32;
                c.blit_alpha_faded(x + 5, (h - s) / 2, icon, s as usize, fade);
                tx = x + 5 + s + 5;
            }
            let n = label_fit(Face::Ui, &win.title, x + bw - BUTTON_GAP - 6 - tx);
            let title: String = win.title.chars().take(n).collect();
            draw_label(&mut c, Face::Ui, tx, 0, h, &title, fg);
        }
        let x = w as i32 - label_width(Face::Mono, &self.status_text) - STATUS_PAD;
        draw_label(&mut c, Face::Mono, x, 0, h, &self.status_text, theme::BAR_TEXT);
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
        self.menu = Some(Menu { items, hover: None, w, h, rows, col_w, buf: vec![0; w * h], glass: Vec::new(), glass_deferred: false, glass_at: 0 });
        self.render_menu();
        self.bar_dirty = true;
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.mark_rows(BAR_H, MENU_Y + m.h + SHADOW_MARGIN as usize);
            self.bar_dirty = true;
        }
    }

    /// 開いているメニューのパネルの矩形
    pub(super) fn menu_rect(&self) -> Option<(i32, i32, i32, i32)> {
        self.menu.as_ref().map(|m| (MENU_X, MENU_Y as i32, m.w as i32, m.h as i32))
    }

    pub(super) fn render_menu(&mut self) {
        let Some(deco) = &self.deco else { return };
        let Some(m) = &mut self.menu else { return };
        let (w, h) = (m.w, m.h);
        let mut c = Canvas::new(&mut m.buf, w, h);
        // すりガラス（blur=on）なら後ろをぼかしたものを地に
        if m.glass.len() == w * h {
            c.blit(0, 0, &m.glass, w);
        } else {
            c.clear(theme::FACE);
        }
        for (i, item) in m.items.iter().enumerate() {
            let x = (i / m.rows * m.col_w) as i32;
            let y = (MENU_PAD + i % m.rows * ITEM_H) as i32;
            let fg = if m.hover == Some(i) {
                c.fill_round_rect(x + 4, y + 1, m.col_w as i32 - 8, ITEM_H as i32 - 2, &deco.corner_small, theme::ACCENT_LIGHT, 255);
                theme::ACCENT_TEXT
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
        deco::panel_row(row, y as i32, rect, &deco.corner, &deco.shadows(true), fill);
    }
}
