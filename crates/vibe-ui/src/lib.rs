//! ウィンドウの中で使う部品のライブラリ
//!
//! 部品（`Widget`）を位置（`Rect`）と一緒に `Ui` に登録し、キー・マウスの入力（`Input`）を
//! `Ui::handle` に渡すと、部品の状態が変わり、アプリが知るべきこと（`Action`: ボタンが押された・
//! 一覧で選んだ など）が返る。描画は `Ui::draw` で 8 ビットの `vibe_gfx::Surface` に黒と白（と使えない字の濃い灰）で行う
//! （明るい地・強調色の青・角の丸い枠。文字は `set_fonts` で渡したなめらかなフォント）。
//!
//! 入力から状態への変化は画面やシステムコールに触らない純粋な処理なので、ホストの `cargo test` で
//! 配置・当たり判定・入力の流れを確かめられる。アプリでは `vibe_rt::gfx::Window` と組み合わせて使う
//! （イベントを `Input` に直して渡し、描いてから `present`）。
//!
//! - フォーカス: Tab / Shift+Tab で順に移る。マウスで押した部品にも移る
//! - ボタン: マウスで押して中で離すか、フォーカスがあるときの Space / Enter で `Clicked`。マウスが乗ると地の色が変わる
//! - テキスト欄: 文字の入力・BackSpace・Delete・←→・Home・End（`Changed`）、Enter で `Submitted`。
//!   クリップボード（`Ui::set_clipboard`）があれば、Ctrl+Insert で写す、Ctrl+X か Shift+Delete で切り取る、
//!   Ctrl+V か Shift+Insert で貼り付ける（Ctrl+C はカーネルがプログラムを止めるのに使うので使わない）
//! - 一覧: クリック・↑↓・PageUp/Down・Home/End で `Selected`、ダブルクリックか Enter で `Activated`。
//!   入りきらなければ右にスクロールバー（溝を押すと 1 画面ずつ、つまみはドラッグ）。項目にアイコンを付けられる
//! - チェックボックス: クリックか Space で `Toggled`
//! - 使わなかったキーは `Action::Key` でそのまま返す（アプリのショートカット用）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dialog;
pub mod draw;
mod scroll;
mod widget;

#[cfg(test)]
mod tests;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

pub use vibe_abi::KeyEvent;
use vibe_abi::key;
use vibe_gfx::Surface;

pub use draw::set_font;
pub use scroll::ScrollBar;
pub use widget::{Align, Button, Checkbox, Label, List, TextField, Widget};

/// 部品の番号（`Ui::add` が返す）
pub type Id = usize;

/// 矩形（ウィンドウの中の座標）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    /// 四辺を `d` だけ内側に寄せる
    pub fn inset(&self, d: i32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2 * d).max(0), (self.h - 2 * d).max(0))
    }

    /// 上から高さ `h` を切り取る。残り（`gap` を空けた下側）と組で返す
    pub fn split_top(&self, h: i32, gap: i32) -> (Rect, Rect) {
        let h = h.min(self.h);
        let rest = (self.h - h - gap).max(0);
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + self.h - rest, self.w, rest))
    }

    /// 下から高さ `h` を切り取る。残り（`gap` を空けた上側）と組で返す
    pub fn split_bottom(&self, h: i32, gap: i32) -> (Rect, Rect) {
        let h = h.min(self.h);
        let rest = (self.h - h - gap).max(0);
        (Rect::new(self.x, self.y + self.h - h, self.w, h), Rect::new(self.x, self.y, self.w, rest))
    }

    /// `cols` × `rows` の升目に分ける（左上から行ごとに）。升目の間は `gap`。割り切れない余りは
    /// 右端・下端の升目に足す
    pub fn grid(&self, cols: i32, rows: i32, gap: i32) -> Vec<Rect> {
        let mut out = Vec::new();
        if cols <= 0 || rows <= 0 {
            return out;
        }
        let cw = (self.w - gap * (cols - 1)) / cols;
        let ch = (self.h - gap * (rows - 1)) / rows;
        for r in 0..rows {
            for c in 0..cols {
                let x = self.x + c * (cw + gap);
                let y = self.y + r * (ch + gap);
                let w = if c == cols - 1 { self.x + self.w - x } else { cw };
                let h = if r == rows - 1 { self.y + self.h - y } else { ch };
                out.push(Rect::new(x, y, w, h));
            }
        }
        out
    }
}

/// 入力
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Key(KeyEvent),
    /// ウィンドウの中の座標と、今押しているボタン（bit0 = 左, bit1 = 右, bit2 = 中）
    Mouse { x: i32, y: i32, buttons: u32 },
}

/// 入力の結果、アプリが知るべきこと
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// ボタンが押された
    Clicked(Id),
    /// テキスト欄の中身が変わった
    Changed(Id),
    /// テキスト欄で Enter
    Submitted(Id),
    /// 一覧で項目を選んだ
    Selected(Id, usize),
    /// 一覧の項目をダブルクリックか Enter で開いた
    Activated(Id, usize),
    /// チェックボックスが変わった
    Toggled(Id, bool),
    /// どの部品も使わなかったキー
    Key(KeyEvent),
}

/// クリップボード（アプリがシステムのものをつなぐ。テストでは偽物）
pub trait Clipboard {
    fn get(&mut self) -> String;
    fn set(&mut self, text: &str);
}

/// ダブルクリックとみなす間隔（タイマの刻み。100 で 1 秒）
pub const DOUBLE_CLICK_TICKS: u32 = 50;

struct Item {
    rect: Rect,
    widget: Widget,
    visible: bool,
}

/// 部品の集まり（ウィンドウ 1 つ分）
pub struct Ui {
    items: Vec<Item>,
    focus: Option<Id>,
    /// マウスのボタンを押した部品（離すまで）
    pressed: Option<Id>,
    /// 一覧のスクロールバーのつまみをドラッグ中: (部品, つまみの上端からのずれ)
    thumb_drag: Option<(Id, i32)>,
    buttons: u32,
    mouse: (i32, i32),
    /// 最後のクリック: (部品, 項目, 時刻)（ダブルクリックの判定）
    last_click: Option<(Id, usize, u32)>,
    clipboard: Option<Box<dyn Clipboard>>,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub const fn new() -> Ui {
        Ui {
            items: Vec::new(),
            focus: None,
            pressed: None,
            thumb_drag: None,
            buttons: 0,
            mouse: (-1, -1),
            last_click: None,
            clipboard: None,
        }
    }

    /// テキスト欄の写す・切り取る・貼り付けるに使うクリップボード
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        self.clipboard = Some(clipboard);
    }

    /// 部品を足す。フォーカスを持てる部品で、まだどれもフォーカスを持っていなければフォーカスを持つ
    pub fn add(&mut self, rect: Rect, widget: impl Into<Widget>) -> Id {
        let id = self.items.len();
        let mut widget = widget.into();
        if let Widget::List(l) = &mut widget {
            l.set_rect(rect);
        }
        self.items.push(Item { rect, widget, visible: true });
        if self.focus.is_none() && self.focusable(id) {
            self.focus = Some(id);
        }
        id
    }

    pub fn rect(&self, id: Id) -> Rect {
        self.items[id].rect
    }

    pub fn set_rect(&mut self, id: Id, rect: Rect) {
        self.items[id].rect = rect;
        if let Widget::List(l) = &mut self.items[id].widget {
            l.set_rect(rect);
        }
    }

    pub fn widget(&self, id: Id) -> &Widget {
        &self.items[id].widget
    }

    pub fn widget_mut(&mut self, id: Id) -> &mut Widget {
        &mut self.items[id].widget
    }

    /// 見せる・隠す（隠した部品は描かず、入力も受けない）
    pub fn set_visible(&mut self, id: Id, visible: bool) {
        self.items[id].visible = visible;
        if !visible && self.focus == Some(id) {
            self.focus = self.next_focusable(id, true);
        }
    }

    /// ラベル・ボタン・テキスト欄・チェックボックスの文字
    pub fn text(&self, id: Id) -> &str {
        match &self.items[id].widget {
            Widget::Label(l) => &l.text,
            Widget::Button(b) => &b.text,
            Widget::TextField(t) => t.text(),
            Widget::Checkbox(c) => &c.text,
            Widget::List(_) => "",
        }
    }

    pub fn set_text(&mut self, id: Id, text: &str) {
        match &mut self.items[id].widget {
            Widget::Label(l) => l.text = String::from(text),
            Widget::Button(b) => b.text = String::from(text),
            Widget::TextField(t) => t.set_text(text),
            Widget::Checkbox(c) => c.text = String::from(text),
            Widget::List(_) => {}
        }
    }

    /// 一覧（`id` が一覧でなければ panic）
    pub fn list(&self, id: Id) -> &List {
        match &self.items[id].widget {
            Widget::List(l) => l,
            _ => panic!("widget {id} is not a list"),
        }
    }

    pub fn list_mut(&mut self, id: Id) -> &mut List {
        match &mut self.items[id].widget {
            Widget::List(l) => l,
            _ => panic!("widget {id} is not a list"),
        }
    }

    pub fn checked(&self, id: Id) -> bool {
        matches!(&self.items[id].widget, Widget::Checkbox(c) if c.checked)
    }

    pub fn focus(&self) -> Option<Id> {
        self.focus
    }

    pub fn set_focus(&mut self, id: Id) {
        if self.focusable(id) {
            self.focus = Some(id);
        }
    }

    /// どの部品にもフォーカスを置かない（キーはすべて `Action::Key` で返る）
    pub fn clear_focus(&mut self) {
        self.focus = None;
    }

    fn focusable(&self, id: Id) -> bool {
        let item = &self.items[id];
        item.visible
            && match &item.widget {
                Widget::Label(_) => false,
                Widget::Button(b) => b.enabled,
                _ => true,
            }
    }

    /// `from` の次（`forward` でなければ前）のフォーカスを持てる部品
    fn next_focusable(&self, from: Id, forward: bool) -> Option<Id> {
        let n = self.items.len();
        (1..=n).map(|i| if forward { (from + i) % n } else { (from + n - i) % n }).find(|&id| self.focusable(id))
    }

    /// (x, y) にある一番上の（後から足した）見えている部品
    pub fn hit(&self, x: i32, y: i32) -> Option<Id> {
        self.items.iter().rposition(|it| it.visible && it.rect.contains(x, y))
    }

    /// 入力を 1 つ処理する。`now` は今の時刻（タイマの刻み。ダブルクリックの判定に使う）
    pub fn handle(&mut self, input: Input, now: u32) -> Option<Action> {
        match input {
            Input::Key(k) => self.handle_key(k),
            Input::Mouse { x, y, buttons } => self.handle_mouse(x, y, buttons, now),
        }
    }

    fn handle_key(&mut self, k: KeyEvent) -> Option<Action> {
        if k.key() == key::TAB && !k.ctrl() && !k.alt() {
            if k.pressed() {
                let from = self.focus.unwrap_or(if k.shift() { 0 } else { self.items.len().saturating_sub(1) });
                if let Some(next) = self.next_focusable(from, !k.shift()) {
                    self.focus = Some(next);
                }
            }
            return None;
        }
        if let Some(id) = self.focus {
            if let Some(result) = self.clipboard_key(id, k) {
                return result;
            }
            if let widget::KeyResult::Used(action) = self.items[id].widget.key(id, k) {
                return action;
            }
        }
        Some(Action::Key(k))
    }

    /// テキスト欄のクリップボードのキー。使ったら Some（中は知らせること）
    fn clipboard_key(&mut self, id: Id, k: KeyEvent) -> Option<Option<Action>> {
        const X: u8 = 0x2D;
        const V: u8 = 0x2F;
        let Widget::TextField(t) = &mut self.items[id].widget else { return None };
        let clip = self.clipboard.as_mut()?;
        let (code, ctrl, shift) = (k.key(), k.ctrl(), k.shift());
        let copy = ctrl && code == key::INSERT;
        let cut = (ctrl && code == X) || (shift && code == key::DELETE);
        let paste = (ctrl && code == V) || (shift && code == key::INSERT);
        if !(copy || cut || paste) {
            return None;
        }
        if !k.pressed() {
            return Some(None);
        }
        if copy || cut {
            clip.set(t.text());
        }
        if cut {
            t.set_text("");
            return Some(Some(Action::Changed(id)));
        }
        if paste {
            let text = clip.get();
            return Some(t.insert_text(&text).then_some(Action::Changed(id)));
        }
        Some(None)
    }

    fn handle_mouse(&mut self, x: i32, y: i32, buttons: u32, now: u32) -> Option<Action> {
        let left = buttons & 1 != 0;
        let was_left = self.buttons & 1 != 0;
        self.buttons = buttons;
        self.mouse = (x, y);
        if left && !was_left {
            let id = self.hit(x, y)?;
            self.pressed = Some(id);
            if self.focusable(id) {
                self.focus = Some(id);
            }
            let rect = self.items[id].rect;
            return match &mut self.items[id].widget {
                Widget::List(list) => match list.press(rect, x, y) {
                    widget::ListPress::Item(i) => {
                        let double = self.last_click.is_some_and(|(cid, ci, t)| {
                            cid == id && ci == i && now.wrapping_sub(t) <= DOUBLE_CLICK_TICKS
                        });
                        if double {
                            self.last_click = None;
                            Some(Action::Activated(id, i))
                        } else {
                            self.last_click = Some((id, i, now));
                            Some(Action::Selected(id, i))
                        }
                    }
                    widget::ListPress::Thumb(grab) => {
                        self.thumb_drag = Some((id, grab));
                        None
                    }
                    widget::ListPress::Scrolled | widget::ListPress::Nothing => None,
                },
                Widget::TextField(t) => {
                    t.click(rect, x);
                    None
                }
                _ => None,
            };
        }
        if left {
            if let Some((id, grab)) = self.thumb_drag {
                let rect = self.items[id].rect;
                if let Widget::List(list) = &mut self.items[id].widget {
                    list.drag_thumb(rect, y - grab);
                }
            }
            return None;
        }
        // 離した
        self.thumb_drag = None;
        let pressed = self.pressed.take()?;
        if was_left && self.items[pressed].rect.contains(x, y) && self.items[pressed].visible {
            match &mut self.items[pressed].widget {
                Widget::Button(b) if b.enabled => return Some(Action::Clicked(pressed)),
                Widget::Checkbox(c) => {
                    c.checked = !c.checked;
                    return Some(Action::Toggled(pressed, c.checked));
                }
                _ => {}
            }
        }
        None
    }

    /// 全部の部品を描く（背景は描かないので、先に塗っておく）
    pub fn draw(&self, s: &mut Surface) {
        let over = self.hit(self.mouse.0, self.mouse.1);
        for (id, it) in self.items.iter().enumerate() {
            if !it.visible {
                continue;
            }
            let focused = self.focus == Some(id);
            let inside = it.rect.contains(self.mouse.0, self.mouse.1);
            let pressed = self.pressed == Some(id) && self.buttons & 1 != 0 && inside;
            let hover = over == Some(id) && self.buttons == 0;
            it.widget.draw(s, it.rect, focused, pressed, hover);
        }
    }

    /// マウスが乗っている部品（ボタンを押していないとき）。変わったら描き直すのに使う
    pub fn hover(&self) -> Option<Id> {
        if self.buttons != 0 {
            return None;
        }
        self.hit(self.mouse.0, self.mouse.1)
    }
}
