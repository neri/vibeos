//! 部品の中身（状態・キーの処理・描画）

use alloc::string::String;
use alloc::vec::Vec;

use vibe_abi::key;
use vibe_gfx::Surface;

use crate::draw::{self, CHAR_W, FACE, FIELD, SELECT, SELECT_TEXT, SHADOW, TEXT, TEXT_DISABLED};
use crate::{Action, Id, KeyEvent, Rect};

/// 一覧の 1 行の高さ・スクロールバーの幅
pub const ROW_H: i32 = 10;
pub const SCROLLBAR_W: i32 = 10;
/// テキスト欄・一覧の枠の内側の余白
const PAD: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

pub enum Widget {
    Label(Label),
    Button(Button),
    TextField(TextField),
    List(List),
    Checkbox(Checkbox),
}

/// 文字を出すだけ（フォーカスを持たない）。`\n` で改行
pub struct Label {
    pub text: String,
    pub align: Align,
    pub color: u8,
    /// 背景の色（None なら塗らない）
    pub bg: Option<u8>,
    /// へこんだ枠を付ける（電卓の表示など）
    pub sunken: bool,
}

impl Label {
    pub fn new(text: &str) -> Label {
        Label { text: String::from(text), align: Align::Left, color: TEXT, bg: None, sunken: false }
    }

    pub fn align(mut self, align: Align) -> Label {
        self.align = align;
        self
    }

    /// 白地のへこんだ枠（表示欄）
    pub fn display(mut self) -> Label {
        self.bg = Some(FIELD);
        self.sunken = true;
        self
    }
}

pub struct Button {
    pub text: String,
    pub enabled: bool,
}

impl Button {
    pub fn new(text: &str) -> Button {
        Button { text: String::from(text), enabled: true }
    }
}

/// 1 行のテキスト欄
pub struct TextField {
    text: String,
    /// カーソルの位置（文字数）
    cursor: usize,
    /// 左端に見えている文字の位置
    scroll: usize,
    pub max_len: usize,
}

impl TextField {
    pub fn new(text: &str) -> TextField {
        let mut t = TextField { text: String::new(), cursor: 0, scroll: 0, max_len: 255 };
        t.set_text(text);
        t
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 中身を置き換え、カーソルを末尾へ（ASCII の表示できる文字だけを残す）
    pub fn set_text(&mut self, text: &str) {
        self.text = text.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).take(self.max_len).collect();
        self.cursor = self.text.len();
        self.scroll = 0;
    }

    /// 幅 `w` の欄で、カーソルが見えるように左端を動かす
    fn fit_scroll(&mut self, w: i32) {
        let visible = visible_chars(w);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor > self.scroll + visible {
            self.scroll = self.cursor - visible;
        }
    }

    pub(crate) fn click(&mut self, rect: Rect, x: i32) {
        let col = ((x - rect.x - PAD + CHAR_W / 2) / CHAR_W).max(0) as usize;
        self.cursor = (self.scroll + col).min(self.text.len());
    }

    fn key(&mut self, id: Id, k: KeyEvent) -> KeyResult {
        if !k.pressed() {
            return KeyResult::Ignored;
        }
        let changed = match (k.key(), k.ch()) {
            (key::ENTER, _) => return KeyResult::Used(Some(Action::Submitted(id))),
            (key::BACKSPACE, _) => {
                if self.cursor == 0 {
                    return KeyResult::Used(None);
                }
                self.cursor -= 1;
                self.text.remove(self.cursor);
                true
            }
            (key::DELETE, _) => {
                if self.cursor >= self.text.len() {
                    return KeyResult::Used(None);
                }
                self.text.remove(self.cursor);
                true
            }
            (key::LEFT, _) => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            (key::RIGHT, _) => {
                self.cursor = (self.cursor + 1).min(self.text.len());
                false
            }
            (key::HOME, _) => {
                self.cursor = 0;
                false
            }
            (key::END, _) => {
                self.cursor = self.text.len();
                false
            }
            (_, Some(c)) if c.is_ascii() && !c.is_ascii_control() && !k.ctrl() && !k.alt() => {
                if self.text.len() >= self.max_len {
                    return KeyResult::Used(None);
                }
                self.text.insert(self.cursor, c);
                self.cursor += 1;
                true
            }
            _ => return KeyResult::Ignored,
        };
        KeyResult::Used(changed.then_some(Action::Changed(id)))
    }
}

fn visible_chars(w: i32) -> usize {
    ((w - 2 * PAD) / CHAR_W).max(1) as usize
}

/// 一覧（スクロールバー付き）
pub struct List {
    items: Vec<String>,
    selected: Option<usize>,
    /// 一番上に見えている項目
    top: usize,
    /// 最後に描いた・触った大きさで見える行数（キーの PageUp/Down に使う）
    rows: usize,
}

/// 一覧をマウスで押した結果
pub(crate) enum ListPress {
    Item(usize),
    /// つまみをつかんだ（つまみの上端からのずれ）
    Thumb(i32),
    /// 溝を押して 1 画面動いた
    Scrolled,
    Nothing,
}

impl List {
    pub fn new(items: Vec<String>) -> List {
        List { items, selected: None, top: 0, rows: 1 }
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// 項目を置き換える（選択と表示位置は先頭に戻す）
    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        self.selected = if self.items.is_empty() { None } else { Some(0) };
        self.top = 0;
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selected.and_then(|i| self.items.get(i)).map(String::as_str)
    }

    pub fn select(&mut self, i: usize) {
        if i < self.items.len() {
            self.selected = Some(i);
            self.show(i);
        }
    }

    pub fn top(&self) -> usize {
        self.top
    }

    /// 大きさ `rect` の一覧に見える行数
    pub fn rows_in(rect: Rect) -> usize {
        ((rect.h - 4) / ROW_H).max(1) as usize
    }

    fn max_top(&self) -> usize {
        self.items.len().saturating_sub(self.rows)
    }

    /// 項目 `i` が見えるように表示位置を動かす
    fn show(&mut self, i: usize) {
        if i < self.top {
            self.top = i;
        } else if i >= self.top + self.rows {
            self.top = i + 1 - self.rows;
        }
    }

    fn has_scrollbar(&self) -> bool {
        self.items.len() > self.rows
    }

    fn scrollbar(&self, rect: Rect) -> (Rect, i32, i32) {
        scrollbar_geometry(rect, self.items.len(), self.rows, self.top)
    }

    /// 見える行数を部品の大きさに合わせる（Ui が部品を足した・動かしたときに呼ぶ）
    pub(crate) fn set_rect(&mut self, rect: Rect) {
        self.rows = Self::rows_in(rect);
        self.top = self.top.min(self.max_top());
    }

    pub(crate) fn press(&mut self, rect: Rect, x: i32, y: i32) -> ListPress {
        self.rows = Self::rows_in(rect);
        if self.has_scrollbar() {
            let (track, ty, th) = self.scrollbar(rect);
            if track.contains(x, y) {
                if y < ty {
                    self.top = self.top.saturating_sub(self.rows);
                } else if y >= ty + th {
                    self.top = (self.top + self.rows).min(self.max_top());
                } else {
                    return ListPress::Thumb(y - ty);
                }
                return ListPress::Scrolled;
            }
        }
        let row = (y - rect.y - 2).div_euclid(ROW_H);
        if row < 0 || row as usize >= self.rows {
            return ListPress::Nothing;
        }
        let i = self.top + row as usize;
        if i >= self.items.len() {
            return ListPress::Nothing;
        }
        self.selected = Some(i);
        ListPress::Item(i)
    }

    /// つまみの上端を `thumb_y` にした位置まで動かす
    pub(crate) fn drag_thumb(&mut self, rect: Rect, thumb_y: i32) {
        self.rows = Self::rows_in(rect);
        let (track, _, th) = self.scrollbar(rect);
        let range = (track.h - th).max(1);
        let pos = (thumb_y - track.y).clamp(0, range);
        self.top = ((pos * self.max_top() as i32 + range / 2) / range) as usize;
    }

    fn key(&mut self, id: Id, k: KeyEvent) -> KeyResult {
        if !k.pressed() {
            return KeyResult::Ignored;
        }
        if self.items.is_empty() {
            return KeyResult::Ignored;
        }
        let last = self.items.len() - 1;
        let cur = self.selected;
        let next = match k.key() {
            key::ENTER => return KeyResult::Used(cur.map(|i| Action::Activated(id, i))),
            key::UP => cur.map_or(0, |i| i.saturating_sub(1)),
            key::DOWN => cur.map_or(0, |i| (i + 1).min(last)),
            key::PAGE_UP => cur.map_or(0, |i| i.saturating_sub(self.rows)),
            key::PAGE_DOWN => cur.map_or(0, |i| (i + self.rows).min(last)),
            key::HOME => 0,
            key::END => last,
            _ => return KeyResult::Ignored,
        };
        self.selected = Some(next);
        self.show(next);
        KeyResult::Used((cur != Some(next)).then_some(Action::Selected(id, next)))
    }
}

/// スクロールバーの溝とつまみ（つまみは (上端, 高さ)）。`n` 項目のうち `rows` 行が見え、先頭が `top`
fn scrollbar_geometry(rect: Rect, n: usize, rows: usize, top: usize) -> (Rect, i32, i32) {
    let track = Rect::new(rect.x + rect.w - 2 - SCROLLBAR_W, rect.y + 2, SCROLLBAR_W, rect.h - 4);
    let h = (track.h * rows as i32 / n.max(1) as i32).clamp(6.min(track.h), track.h);
    let range = (track.h - h).max(0);
    let max_top = n.saturating_sub(rows).max(1) as i32;
    let y = track.y + range * top.min(max_top as usize) as i32 / max_top;
    (track, y, h)
}

pub struct Checkbox {
    pub text: String,
    pub checked: bool,
}

impl Checkbox {
    pub fn new(text: &str, checked: bool) -> Checkbox {
        Checkbox { text: String::from(text), checked }
    }
}

macro_rules! impl_from {
    ($($t:ident),*) => {
        $(impl From<$t> for Widget {
            fn from(w: $t) -> Widget {
                Widget::$t(w)
            }
        })*
    };
}

impl_from!(Label, Button, TextField, List, Checkbox);

pub(crate) enum KeyResult {
    /// 使った（アプリに知らせることがあれば Some）
    Used(Option<Action>),
    Ignored,
}

impl Widget {
    pub(crate) fn key(&mut self, id: Id, k: KeyEvent) -> KeyResult {
        match self {
            Widget::Button(b) if b.enabled => match (k.key(), k.pressed()) {
                (key::SPACE | key::ENTER, true) => KeyResult::Used(Some(Action::Clicked(id))),
                _ => KeyResult::Ignored,
            },
            Widget::TextField(t) => t.key(id, k),
            Widget::List(l) => l.key(id, k),
            Widget::Checkbox(c) => match (k.key(), k.pressed()) {
                (key::SPACE, true) => {
                    c.checked = !c.checked;
                    KeyResult::Used(Some(Action::Toggled(id, c.checked)))
                }
                _ => KeyResult::Ignored,
            },
            _ => KeyResult::Ignored,
        }
    }

    pub(crate) fn draw(&self, s: &mut Surface, r: Rect, focused: bool, pressed: bool) {
        match self {
            Widget::Label(l) => {
                if let Some(bg) = l.bg {
                    s.fill_rect(r.x, r.y, r.w, r.h, bg);
                }
                if l.sunken {
                    draw::bevel(s, r, false);
                }
                let lines: Vec<&str> = l.text.split('\n').collect();
                let total = lines.len() as i32 * draw::LINE_H;
                let mut y = r.y + (r.h - total) / 2 + 1;
                for line in lines {
                    draw::text_aligned(s, r.inset(PAD), y, line, l.align, l.color);
                    y += draw::LINE_H;
                }
            }
            Widget::Button(b) => {
                s.fill_rect(r.x, r.y, r.w, r.h, FACE);
                let inner = if focused {
                    s.rect(r.x, r.y, r.w, r.h, TEXT);
                    r.inset(1)
                } else {
                    r
                };
                draw::bevel(s, inner, !pressed);
                let shift = pressed as i32;
                let color = if b.enabled { TEXT } else { TEXT_DISABLED };
                let y = r.y + (r.h - draw::CHAR_H) / 2 + shift;
                draw::text_aligned(s, Rect::new(r.x + shift, r.y, r.w, r.h), y, &b.text, Align::Center, color);
            }
            Widget::TextField(t) => {
                s.fill_rect(r.x, r.y, r.w, r.h, FIELD);
                draw::bevel(s, r, false);
                let mut t2 = TextField { text: String::new(), cursor: t.cursor, scroll: t.scroll, max_len: t.max_len };
                t2.fit_scroll(r.w);
                let visible = visible_chars(r.w);
                let shown: String = t.text.chars().skip(t2.scroll).take(visible).collect();
                let y = r.y + (r.h - draw::CHAR_H) / 2;
                draw::text(s, r.x + PAD, y, &shown, TEXT);
                if focused {
                    let cx = r.x + PAD + (t.cursor - t2.scroll) as i32 * CHAR_W;
                    s.vline(cx.min(r.x + r.w - 2), y - 1, draw::CHAR_H + 2, TEXT);
                }
            }
            Widget::List(l) => {
                s.fill_rect(r.x, r.y, r.w, r.h, FIELD);
                draw::bevel(s, r, false);
                let rows = List::rows_in(r);
                let top = l.top.min(l.items.len().saturating_sub(rows));
                let scroll = l.items.len() > rows;
                let text_w = r.w - 4 - if scroll { SCROLLBAR_W } else { 0 };
                for (row, i) in (top..l.items.len()).take(rows).enumerate() {
                    let y = r.y + 2 + row as i32 * ROW_H;
                    let color = if l.selected == Some(i) {
                        s.fill_rect(r.x + 2, y, text_w, ROW_H, if focused { SELECT } else { SHADOW });
                        SELECT_TEXT
                    } else {
                        TEXT
                    };
                    let room = ((text_w - 4) / CHAR_W).max(0) as usize;
                    let name: String = l.items[i].chars().take(room).collect();
                    draw::text(s, r.x + 4, y + 1, &name, color);
                }
                if scroll {
                    let (track, ty, th) = scrollbar_geometry(r, l.items.len(), rows, top);
                    s.fill_rect(track.x, track.y, track.w, track.h, SHADOW);
                    let thumb = Rect::new(track.x, ty, track.w, th);
                    s.fill_rect(thumb.x, thumb.y, thumb.w, thumb.h, FACE);
                    draw::bevel(s, thumb, true);
                }
                if focused && l.selected.is_none() {
                    s.rect(r.x + 2, r.y + 2, text_w, ROW_H, TEXT);
                }
            }
            Widget::Checkbox(c) => {
                let bx = Rect::new(r.x, r.y + (r.h - 10) / 2, 10, 10);
                s.fill_rect(bx.x, bx.y, bx.w, bx.h, FIELD);
                draw::bevel(s, bx, false);
                if c.checked {
                    s.line(bx.x + 2, bx.y + 5, bx.x + 4, bx.y + 7, TEXT);
                    s.line(bx.x + 4, bx.y + 7, bx.x + 7, bx.y + 2, TEXT);
                }
                let y = r.y + (r.h - draw::CHAR_H) / 2;
                let end = draw::text(s, r.x + 14, y, &c.text, TEXT);
                if focused {
                    s.hline(r.x + 14, y + draw::CHAR_H, end - r.x - 14, TEXT);
                }
            }
        }
    }
}
