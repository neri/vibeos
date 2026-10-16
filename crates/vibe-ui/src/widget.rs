//! 部品の中身（状態・キーの処理・描画）

use alloc::string::String;
use alloc::vec::Vec;

use vibe_abi::key;
use vibe_gfx::Surface;
use vibe_gfx::icons::TRANSPARENT;

use crate::draw::{self, DARK, FACE, FIELD, FOCUS, SELECT, SELECT_INACTIVE, SELECT_TEXT, TEXT, TEXT_DISABLED, Text};
use crate::{Action, Id, KeyEvent, Rect};

/// 一覧の 1 行の高さ・スクロールバーの幅
pub const ROW_H: i32 = 20;
pub const SCROLLBAR_W: i32 = 10;
/// テキスト欄・一覧の枠の内側の余白
const PAD: i32 = 6;
/// 一覧の項目のアイコンの大きさ
const ICON: i32 = 16;

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
    /// へこんだ欄にする（電卓の表示など）
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

    /// 白い地のへこんだ欄（表示欄）
    pub fn display(mut self) -> Label {
        self.bg = Some(FIELD);
        self.sunken = true;
        self
    }
}

pub struct Button {
    pub text: String,
    pub enabled: bool,
    /// 既定のボタン（黒い太い縁。ダイアログの Enter で押されるもの）
    pub primary: bool,
}

impl Button {
    pub fn new(text: &str) -> Button {
        Button { text: String::from(text), enabled: true, primary: false }
    }

    /// 既定のボタンにする
    pub fn primary(mut self) -> Button {
        self.primary = true;
        self
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
    /// 全体を選んでいる（次に打った文字で置き換える。BackSpace・Delete で全部消す）
    all_selected: bool,
}

impl TextField {
    pub fn new(text: &str) -> TextField {
        let mut t = TextField { text: String::new(), cursor: 0, scroll: 0, max_len: 255, all_selected: false };
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
        self.all_selected = false;
    }

    /// カーソルの位置に文字を入れる（全体を選んでいれば置き換える。表示できない文字は除き、
    /// 改行より後ろは捨てる）。何か変わったら true
    pub fn insert_text(&mut self, text: &str) -> bool {
        if self.all_selected {
            self.set_text("");
        }
        let mut changed = false;
        for c in text.chars().take_while(|&c| c != '\n') {
            if c.is_ascii() && !c.is_ascii_control() && self.text.len() < self.max_len {
                self.text.insert(self.cursor, c);
                self.cursor += 1;
                changed = true;
            }
        }
        changed
    }

    /// 全体を選ぶ（次に打った文字で置き換わる）
    pub fn select_all(&mut self) {
        self.all_selected = !self.text.is_empty();
    }

    pub fn all_selected(&self) -> bool {
        self.all_selected
    }

    /// 幅 `w` の欄で、カーソルが見えるように左端を動かす
    fn fit_scroll(&mut self, w: i32, t: Text) {
        let room = (w - 2 * PAD).max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        while self.scroll < self.cursor && t.prefix_width(&self.text, self.cursor) - t.prefix_width(&self.text, self.scroll) > room {
            self.scroll += 1;
        }
    }

    pub(crate) fn click(&mut self, rect: Rect, x: i32) {
        self.all_selected = false;
        let t = draw::text();
        let rest: String = self.text.chars().skip(self.scroll).collect();
        self.cursor = (self.scroll + t.index_at(&rest, x - rect.x - PAD)).min(self.text.len());
    }

    fn key(&mut self, id: Id, k: KeyEvent) -> KeyResult {
        if !k.pressed() {
            return KeyResult::Ignored;
        }
        if self.all_selected {
            match (k.key(), k.ch()) {
                (key::BACKSPACE | key::DELETE, _) => {
                    self.set_text("");
                    return KeyResult::Used(Some(Action::Changed(id)));
                }
                (key::ENTER, _) => {}
                (_, Some(c)) if c.is_ascii() && !c.is_ascii_control() && !k.ctrl() && !k.alt() => self.set_text(""),
                _ => self.all_selected = false,
            }
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

/// 一覧（スクロールバー付き）
pub struct List {
    items: Vec<String>,
    /// 項目のアイコン（16 色の 16x16 の色番号、透明は `TRANSPARENT`。無ければアイコンの場所を空けない）
    icons: Vec<Option<&'static [u8]>>,
    /// 列: 項目の文字をタブで分け、i 番目を (左端, 寄せ方) の列に描く（左端は文字の場所の左からの位置。
    /// 列の右端は次の列の左端か、文字の場所の右端）。空なら 1 列
    columns: Vec<(i32, Align)>,
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
        List { items, icons: Vec::new(), columns: Vec::new(), selected: None, top: 0, rows: 1 }
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// 項目を置き換える（選択と表示位置は先頭に戻す）
    pub fn set_items(&mut self, items: Vec<String>) {
        self.items = items;
        self.icons.clear();
        self.selected = if self.items.is_empty() { None } else { Some(0) };
        self.top = 0;
    }

    /// 列（項目の文字をタブで分けたもの）の (左端, 寄せ方)。左端は文字の場所の左端から
    pub fn set_columns(&mut self, columns: Vec<(i32, Align)>) {
        self.columns = columns;
    }

    /// 項目のアイコン（`items` と同じ順。`set_items` で消える）
    pub fn set_icons(&mut self, icons: Vec<Option<&'static [u8]>>) {
        self.icons = icons;
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

    /// 描く。`hover` はマウスが乗っている（ボタンを押していない）とき（16 色の見た目では使わない）
    pub(crate) fn draw(&self, s: &mut Surface, r: Rect, focused: bool, pressed: bool, _hover: bool) {
        let t = draw::text();
        match self {
            Widget::Label(l) => {
                if l.sunken {
                    s.fill_rect(r.x, r.y, r.w, r.h, l.bg.unwrap_or(FIELD));
                    draw::sunken(s, r);
                } else if let Some(bg) = l.bg {
                    s.fill_rect(r.x, r.y, r.w, r.h, bg);
                }
                let lines: Vec<&str> = l.text.split('\n').collect();
                let lh = t.height() + 2;
                let total = lines.len() as i32 * lh;
                let mut y = r.y + (r.h - total) / 2;
                let inner = Rect::new(r.x + PAD, 0, r.w - 2 * PAD, lh);
                for line in lines {
                    t.draw_in(s, Rect::new(inner.x, y, inner.w, lh), line, l.align, l.color);
                    y += lh;
                }
            }
            Widget::Button(b) => {
                let down = b.enabled && pressed;
                s.fill_rect(r.x, r.y, r.w, r.h, FACE);
                // 既定のボタンは外側に黒い縁を足す
                let body = if b.primary && b.enabled {
                    s.rect(r.x, r.y, r.w, r.h, DARK);
                    Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2)
                } else {
                    r
                };
                if down {
                    draw::pressed(s, body);
                } else {
                    draw::raised(s, body);
                }
                // 押している間は文字を 1 ピクセル右下へ
                let off = if down { 1 } else { 0 };
                let inner = Rect::new(body.x + off, body.y + off, body.w, body.h);
                let color = if b.enabled { TEXT } else { TEXT_DISABLED };
                t.draw_in(s, inner, &b.text, Align::Center, color);
                if focused {
                    let w = t.width(&b.text).min(body.w - 8);
                    let fr = Rect::new(inner.x + (body.w - w) / 2 - 3, inner.y + (body.h - t.height()) / 2 - 1, w + 6, t.height() + 2);
                    draw::focus_rect(s, fr);
                }
            }
            Widget::TextField(f) => {
                s.fill_rect(r.x, r.y, r.w, r.h, FIELD);
                draw::sunken(s, r);
                let mut f2 = TextField {
                    text: f.text.clone(),
                    cursor: f.cursor,
                    scroll: f.scroll,
                    max_len: f.max_len,
                    all_selected: false,
                };
                f2.fit_scroll(r.w, t);
                let rest: String = f.text.chars().skip(f2.scroll).collect();
                let n = t.fit(&rest, r.w - 2 * PAD);
                let shown: String = rest.chars().take(n).collect();
                let top = r.y + (r.h - t.height()) / 2;
                let selected = f.all_selected && focused;
                if selected {
                    s.fill_rect(r.x + PAD - 1, top, t.width(&shown) + 2, t.height(), SELECT);
                }
                t.draw(s, r.x + PAD, top, &shown, if selected { SELECT_TEXT } else { TEXT });
                if focused && !f.all_selected {
                    let cx = r.x + PAD + t.prefix_width(&rest, f.cursor - f2.scroll);
                    s.vline(cx.min(r.x + r.w - 3), top, t.height(), TEXT);
                }
            }
            Widget::List(l) => {
                s.fill_rect(r.x, r.y, r.w, r.h, FIELD);
                draw::sunken(s, r);
                let rows = List::rows_in(r);
                let top = l.top.min(l.items.len().saturating_sub(rows));
                let scroll = l.items.len() > rows;
                let text_w = r.w - 4 - if scroll { SCROLLBAR_W + 2 } else { 0 };
                for (row, i) in (top..l.items.len()).take(rows).enumerate() {
                    let y = r.y + 2 + row as i32 * ROW_H;
                    let color = if l.selected == Some(i) {
                        let bg = if focused { SELECT } else { SELECT_INACTIVE };
                        s.fill_rect(r.x + 2, y, text_w, ROW_H, bg);
                        if focused { SELECT_TEXT } else { TEXT }
                    } else {
                        TEXT
                    };
                    let mut x = r.x + PAD;
                    if let Some(icon) = l.icons.get(i).copied().flatten() {
                        s.blit(x, y + (ROW_H - ICON) / 2, icon, ICON as usize, Some(TRANSPARENT));
                        x += ICON + 6;
                    }
                    let area = Rect::new(x, y, r.x + text_w - PAD - x, ROW_H);
                    if l.columns.is_empty() {
                        t.draw_in(s, area, &l.items[i], Align::Left, color);
                    } else {
                        for (n, part) in l.items[i].split('\t').enumerate().take(l.columns.len()) {
                            let (x0, align) = l.columns[n];
                            let x1 = l.columns.get(n + 1).map_or(area.w, |c| c.0 - 4);
                            t.draw_in(s, Rect::new(area.x + x0, y, x1 - x0, ROW_H), part, align, color);
                        }
                    }
                }
                if scroll {
                    let (track, ty, th) = scrollbar_geometry(r, l.items.len(), rows, top);
                    draw_scrollbar(s, track, ty, th);
                }
                if focused && l.selected.is_none() {
                    draw::focus_rect(s, Rect::new(r.x + 2, r.y + 2, text_w, ROW_H));
                }
            }
            Widget::Checkbox(cb) => {
                let bx = Rect::new(r.x, r.y + (r.h - 13) / 2, 13, 13);
                s.fill_rect(bx.x, bx.y, bx.w, bx.h, FIELD);
                draw::sunken(s, bx);
                if cb.checked {
                    // チェックの印（黒、2 ピクセルの太さ）
                    for d in 0..2 {
                        s.line(bx.x + 3, bx.y + 5 + d, bx.x + 5, bx.y + 7 + d, FOCUS);
                        s.line(bx.x + 5, bx.y + 7 + d, bx.x + 9, bx.y + 3 + d, FOCUS);
                    }
                }
                let label = Rect::new(r.x + 20, r.y, r.w - 20, r.h);
                t.draw_in(s, label, &cb.text, Align::Left, TEXT);
                if focused {
                    let w = t.width(&cb.text).min(label.w);
                    draw::focus_rect(s, Rect::new(label.x - 2, label.y + (label.h - t.height()) / 2 - 1, w + 4, t.height() + 2));
                }
            }
        }
    }
}

/// スクロールバー: 溝（明るい灰色）と、出っ張った立体のつまみ
pub(crate) fn draw_scrollbar(s: &mut Surface, track: Rect, ty: i32, th: i32) {
    s.fill_rect(track.x, track.y, track.w, track.h, FACE);
    let thumb = Rect::new(track.x, ty, track.w, th);
    s.fill_rect(thumb.x, thumb.y, thumb.w, thumb.h, FACE);
    draw::raised(s, thumb);
}
