//! デスクトップの上端のバーとメニュー
//!
//! バー: 左に「VibeOS」（メニューのボタン）、続けて開いているウィンドウのボタン（作った順）、
//! 右に状態の表示（ステータスバーのスレッドが `set_status_text` で渡す文字列）。
//! ウィンドウのボタンは、フォーカスのあるものは明るい青、最小化したものは塗らずに灰色の文字。
//!
//! メニュー: 「VibeOS」を押すとバーの下に開く。項目は "Terminal" と /BIN のアプリ（開くたびに読む）。
//! 項目を押すと起動する（ロックの外で。`super::launch`）。どこを押してもメニューは閉じる。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_font::FONT_8X8;
use vibe_gfx::{Surface, palette};

use super::{BAR_BUF, BAR_H, Screen, WinId};

const BAR_BG: u8 = palette::BLUE;
/// 「VibeOS」のボタンの幅（文字は x = 8 から）
const MENU_BUTTON_W: i32 = 60;
const BUTTON_MAX_W: i32 = 96;
const BUTTON_MIN_W: i32 = 24;
const BUTTON_GAP: i32 = 2;
const BUTTON: u8 = palette::DARK_GRAY;
const BUTTON_FOCUSED: u8 = palette::LIGHT_BLUE;
/// メニューの 1 項目の高さと上下の余白
const ITEM_H: usize = 10;
const MENU_PAD: usize = 2;
const MENU_BG: u8 = palette::LIGHT_GRAY;
const MENU_HOVER: u8 = palette::BLUE;

pub(super) struct Menu {
    pub(super) items: Vec<String>,
    hover: Option<usize>,
    w: usize,
    h: usize,
    buf: Vec<u8>,
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
        let right = self.width as i32 - (self.status_text.len() * FONT_8X8.width) as i32 - 4;
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

    /// バーを BAR_BUF に描き直す
    pub(super) fn render_bar(&mut self) {
        self.bar_dirty = false;
        let w = self.width;
        let font = &FONT_8X8;
        let focused = self.focused();
        let buttons = self.bar_buttons();
        // SAFETY: BAR_BUF は SCREEN のロックの中でだけ触る
        let bar = unsafe { &mut (&mut *BAR_BUF.as_ptr())[..w * BAR_H] };
        let mut s = Surface::new(bar, w, BAR_H);
        s.clear(BAR_BG);
        if self.menu.is_some() {
            s.fill_rect(0, 0, MENU_BUTTON_W - BUTTON_GAP, BAR_H as i32, BUTTON_FOCUSED);
        }
        s.draw_text(font, 8, 1, "VibeOS", palette::YELLOW, None);
        for (id, x, bw) in buttons {
            let win = &self.windows[&id];
            let (bg, fg) = if win.minimized {
                (None, palette::LIGHT_GRAY)
            } else if Some(id) == focused {
                (Some(BUTTON_FOCUSED), palette::WHITE)
            } else {
                (Some(BUTTON), palette::WHITE)
            };
            if let Some(bg) = bg {
                s.fill_rect(x, 0, bw - BUTTON_GAP, BAR_H as i32, bg);
            }
            let room = ((bw - BUTTON_GAP - 6) / font.width as i32).max(0) as usize;
            let title: String = win.title.chars().take(room).collect();
            s.draw_text(font, x + 3, 1, &title, fg, None);
        }
        let x = w as i32 - (self.status_text.len() * font.width) as i32;
        s.draw_text(font, x, 1, &self.status_text, palette::WHITE, None);
        self.mark_rows(0, BAR_H);
    }

    pub(super) fn open_menu(&mut self, items: Vec<String>) {
        let longest = items.iter().map(String::len).max().unwrap_or(0);
        let w = (longest * FONT_8X8.width + 16).max(96).min(self.width);
        let h = (items.len() * ITEM_H + 2 * MENU_PAD).min(self.height - BAR_H);
        self.menu = Some(Menu { items, hover: None, w, h, buf: vec![0; w * h] });
        self.render_menu();
        self.bar_dirty = true;
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.mark_rows(BAR_H, BAR_H + m.h);
            self.bar_dirty = true;
        }
    }

    fn render_menu(&mut self) {
        let Some(m) = &mut self.menu else { return };
        let (w, h) = (m.w, m.h);
        let mut s = Surface::new(&mut m.buf, w, h);
        s.clear(MENU_BG);
        for (i, item) in m.items.iter().enumerate() {
            let y = (MENU_PAD + i * ITEM_H) as i32;
            let fg = if m.hover == Some(i) {
                s.fill_rect(1, y, w as i32 - 2, ITEM_H as i32, MENU_HOVER);
                palette::WHITE
            } else {
                palette::BLACK
            };
            s.draw_text(&FONT_8X8, 8, y + 1, item, fg, None);
        }
        s.rect(0, 0, w as i32, h as i32, palette::DARK_GRAY);
        self.mark_rows(BAR_H, BAR_H + h);
    }

    /// (x, y) にあるメニューの項目。メニューの外なら None、中でも項目でなければ Some(None)
    pub(super) fn menu_item_at(&self, x: i32, y: i32) -> Option<Option<usize>> {
        let m = self.menu.as_ref()?;
        let (mx, my) = (x, y - BAR_H as i32);
        if mx < 0 || my < 0 || mx >= m.w as i32 || my >= m.h as i32 {
            return None;
        }
        let i = (my - MENU_PAD as i32).div_euclid(ITEM_H as i32);
        Some((my >= MENU_PAD as i32 && (i as usize) < m.items.len()).then_some(i as usize))
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

    /// 重ね合わせ: メニューの行を row に重ねる
    pub(super) fn draw_menu_row(&self, y: usize, row: &mut [u32]) {
        let Some(m) = &self.menu else { return };
        if y < BAR_H || y >= BAR_H + m.h {
            return;
        }
        let r = y - BAR_H;
        let n = m.w.min(row.len());
        super::convert_row(&mut row[..n], &m.buf[r * m.w..r * m.w + n], &self.std_lut);
    }
}
