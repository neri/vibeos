//! デスクトップの上端のバーとメニュー（並べ方と当たり判定。描くのは draw.rs）
//!
//! バー: 左に「VibeOS」（メニューのボタン）、続けて開いているウィンドウのボタン（作った順）、
//! 右に状態の表示（ステータスバーのスレッドが `set_status_text` で渡す文字列）。
//! ボタンを押すと前に出す（最小化していれば戻す）。フォーカスのあるウィンドウのボタンなら最小化する。
//!
//! メニュー: 「VibeOS」を押すとバーの下に開く。項目は "Terminal" と /BIN のアプリ（開くたびに読む）。
//! 画面の高さに入りきらなければ、何列かに分けて並べる（上から下、左から右の順）。
//! 項目を押すと起動する（ロックの外で。`super::launch`）。どこを押してもメニューは閉じる。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::draw::{MENU_ITEM_H, text_width};
use super::{BAR_H, Screen, WinId};

/// 「VibeOS」のボタンの幅
pub(super) const MENU_BUTTON_W: i32 = 72;
const BUTTON_MAX_W: i32 = 140;
const BUTTON_MIN_W: i32 = 32;
pub(super) const BUTTON_GAP: i32 = 2;
/// 状態の表示の右の余白
pub(super) const STATUS_PAD: i32 = 8;
/// メニューのパネルの左端と上下の余白、画面の下に空ける高さ
pub(super) const MENU_X: i32 = 2;
pub(super) const MENU_PAD: usize = 2;
const MENU_BOTTOM_GAP: usize = 8;
/// メニューの項目の名前の位置（列の左端から）と、列の幅の最小
const ITEM_TEXT_X: usize = 24;
const COLUMN_MIN_W: usize = 130;

/// メニューのパネルの上端（バーの下）
pub(super) fn menu_y() -> usize {
    BAR_H + 2
}

pub(super) struct Menu {
    pub(super) items: Vec<String>,
    pub(super) hover: Option<usize>,
    pub(super) w: usize,
    pub(super) h: usize,
    /// 1 列の項目の数と列の幅
    pub(super) rows: usize,
    pub(super) col_w: usize,
    /// 中身（色番号、w × h）
    pub(super) buf: Vec<u8>,
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
    pub(super) fn bar_buttons(&self) -> Vec<(WinId, i32, i32)> {
        let right = self.width as i32 - text_width(&self.status_text) - 2 * STATUS_PAD;
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

    pub(super) fn open_menu(&mut self, items: Vec<String>) {
        let longest = items.iter().map(|i| text_width(i)).max().unwrap_or(0);
        let col_w = (longest as usize + ITEM_TEXT_X + 20).max(COLUMN_MIN_W);
        let room = self.height - menu_y() - 2 * MENU_PAD - MENU_BOTTOM_GAP;
        let rows = (room / MENU_ITEM_H).clamp(1, items.len().max(1));
        let cols = items.len().div_ceil(rows).max(1);
        let w = (cols * col_w).min(self.width);
        let h = rows * MENU_ITEM_H + 2 * MENU_PAD;
        self.menu = Some(Menu { items, hover: None, w, h, rows, col_w, buf: vec![0; w * h] });
        self.render_menu();
        self.bar_dirty = true;
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.mark_rows(BAR_H, menu_y() + m.h);
            self.bar_dirty = true;
        }
    }

    /// (x, y) にあるメニューの項目。メニューの外なら None、中でも項目でなければ Some(None)
    pub(super) fn menu_item_at(&self, x: i32, y: i32) -> Option<Option<usize>> {
        let m = self.menu.as_ref()?;
        let (mx, my) = (x - MENU_X, y - menu_y() as i32);
        if mx < 0 || my < 0 || mx >= m.w as i32 || my >= m.h as i32 {
            return None;
        }
        let row = (my - MENU_PAD as i32).div_euclid(MENU_ITEM_H as i32);
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
}
