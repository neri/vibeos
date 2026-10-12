use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_gfx::Surface;

use crate::dialog::{Dialog, wrap};
use crate::draw::{FACE, FIELD, SELECT, TEXT};
use crate::widget::ROW_H;
use crate::*;

fn press(ch: Option<char>, code: u16) -> Input {
    Input::Key(KeyEvent(KeyEvent::encode(ch, code, true, false, false, false)))
}

fn release(ch: Option<char>, code: u16) -> Input {
    Input::Key(KeyEvent(KeyEvent::encode(ch, code, false, false, false, false)))
}

fn shift_tab() -> Input {
    Input::Key(KeyEvent(KeyEvent::encode(Some('\t'), 0x0F, true, true, false, false)))
}

/// set 1 のコード（E0 付きは 0xE0xx）
const TAB: u16 = 0x0F;
const ENTER: u16 = 0x1C;
const SPACE: u16 = 0x39;
const BACKSPACE: u16 = 0x0E;
const LEFT: u16 = 0xE04B;
const HOME: u16 = 0xE047;
const END: u16 = 0xE04F;
const DELETE: u16 = 0xE053;
const UP: u16 = 0xE048;
const DOWN: u16 = 0xE050;
const PAGE_DOWN: u16 = 0xE051;
const ESCAPE: u16 = 0x01;

fn typed(c: char) -> Input {
    press(Some(c), 0x1E) // コードはどれでもよい（文字で判断する）
}

fn mouse(x: i32, y: i32, buttons: u32) -> Input {
    Input::Mouse { x, y, buttons }
}

/// (x, y) を左クリックしたときの結果（押す・離すの両方の結果）
fn click(ui: &mut Ui, x: i32, y: i32, now: u32) -> Vec<Action> {
    let mut out = Vec::new();
    out.extend(ui.handle(mouse(x, y, 1), now));
    out.extend(ui.handle(mouse(x, y, 0), now));
    out
}

fn items(n: usize) -> Vec<String> {
    (0..n).map(|i| alloc::format!("item{i}")).collect()
}

#[test]
fn grid_covers_the_rect() {
    let cells = Rect::new(10, 20, 100, 50).grid(4, 2, 2);
    assert_eq!(cells.len(), 8);
    assert_eq!(cells[0], Rect::new(10, 20, 23, 24));
    assert_eq!(cells[1], Rect::new(35, 20, 23, 24));
    // 右端・下端の升目は余りを足して端までそろえる
    assert_eq!(cells[3].x + cells[3].w, 110);
    assert_eq!(cells[7].y + cells[7].h, 70);
    assert_eq!(cells[4].y, 46);
}

#[test]
fn split_and_inset() {
    let r = Rect::new(0, 0, 100, 100);
    let (top, rest) = r.split_top(20, 4);
    assert_eq!(top, Rect::new(0, 0, 100, 20));
    assert_eq!(rest, Rect::new(0, 24, 100, 76));
    let (bottom, rest) = r.split_bottom(30, 0);
    assert_eq!(bottom, Rect::new(0, 70, 100, 30));
    assert_eq!(rest, Rect::new(0, 0, 100, 70));
    assert_eq!(r.inset(5), Rect::new(5, 5, 90, 90));
    assert!(r.contains(0, 0) && r.contains(99, 99) && !r.contains(100, 50));
}

#[test]
fn button_clicks_only_when_released_inside() {
    let mut ui = Ui::new();
    let b = ui.add(Rect::new(10, 10, 40, 16), Button::new("OK"));
    assert_eq!(click(&mut ui, 20, 15, 0), vec![Action::Clicked(b)]);
    // 中で押して外で離すと押したことにならない
    assert_eq!(ui.handle(mouse(20, 15, 1), 0), None);
    assert_eq!(ui.handle(mouse(80, 15, 1), 0), None);
    assert_eq!(ui.handle(mouse(80, 15, 0), 0), None);
    // 外で押して中で離しても押したことにならない
    ui.handle(mouse(80, 15, 1), 0);
    assert_eq!(ui.handle(mouse(20, 15, 0), 0), None);
    // 使えないボタン
    if let Widget::Button(btn) = ui.widget_mut(b) {
        btn.enabled = false;
    }
    assert!(click(&mut ui, 20, 15, 0).is_empty());
}

#[test]
fn button_with_keyboard() {
    let mut ui = Ui::new();
    let a = ui.add(Rect::new(0, 0, 40, 16), Button::new("A"));
    let b = ui.add(Rect::new(50, 0, 40, 16), Button::new("B"));
    assert_eq!(ui.focus(), Some(a));
    assert_eq!(ui.handle(press(Some(' '), SPACE), 0), Some(Action::Clicked(a)));
    ui.handle(press(Some('\t'), TAB), 0);
    assert_eq!(ui.handle(press(Some('\n'), ENTER), 0), Some(Action::Clicked(b)));
    // 離したイベントは使わない
    assert!(matches!(ui.handle(release(Some(' '), SPACE), 0), Some(Action::Key(_))));
}

#[test]
fn tab_moves_focus_and_skips_labels() {
    let mut ui = Ui::new();
    let _l = ui.add(Rect::new(0, 0, 10, 10), Label::new("x"));
    let t = ui.add(Rect::new(0, 10, 80, 14), TextField::new(""));
    let _l2 = ui.add(Rect::new(0, 30, 10, 10), Label::new("y"));
    let b = ui.add(Rect::new(0, 40, 40, 16), Button::new("Go"));
    let c = ui.add(Rect::new(0, 60, 80, 12), Checkbox::new("opt", false));
    assert_eq!(ui.focus(), Some(t));
    ui.handle(press(Some('\t'), TAB), 0);
    assert_eq!(ui.focus(), Some(b));
    ui.handle(press(Some('\t'), TAB), 0);
    assert_eq!(ui.focus(), Some(c));
    ui.handle(press(Some('\t'), TAB), 0);
    assert_eq!(ui.focus(), Some(t), "wraps around");
    ui.handle(shift_tab(), 0);
    assert_eq!(ui.focus(), Some(c));
    // 押した部品にフォーカスが移る
    click(&mut ui, 5, 45, 0);
    assert_eq!(ui.focus(), Some(b));
    // 隠すと次へ移る
    ui.set_visible(b, false);
    assert_eq!(ui.focus(), Some(c));
    assert!(click(&mut ui, 5, 45, 0).is_empty());
}

#[test]
fn text_field_editing() {
    let mut ui = Ui::new();
    let t = ui.add(Rect::new(0, 0, 80, 14), TextField::new(""));
    for c in "helo".chars() {
        assert_eq!(ui.handle(typed(c), 0), Some(Action::Changed(t)));
    }
    ui.handle(press(None, LEFT), 0);
    assert_eq!(ui.handle(typed('l'), 0), Some(Action::Changed(t)));
    assert_eq!(ui.text(t), "hello");
    ui.handle(press(None, HOME), 0);
    assert_eq!(ui.handle(press(None, DELETE), 0), Some(Action::Changed(t)));
    assert_eq!(ui.text(t), "ello");
    // 先頭での BackSpace は何もしない（変わらない）
    assert_eq!(ui.handle(press(Some('\x08'), BACKSPACE), 0), None);
    ui.handle(press(None, END), 0);
    ui.handle(press(Some('\x08'), BACKSPACE), 0);
    assert_eq!(ui.text(t), "ell");
    assert_eq!(ui.handle(press(Some('\n'), ENTER), 0), Some(Action::Submitted(t)));
    // 使わないキー（↑）はアプリへ
    assert!(matches!(ui.handle(press(None, UP), 0), Some(Action::Key(_))));
    ui.set_text(t, "abc");
    assert_eq!(ui.text(t), "abc");
    if let Widget::TextField(f) = ui.widget(t) {
        assert_eq!(f.cursor(), 3);
    }
}

#[test]
fn text_field_click_moves_cursor_and_respects_max_len() {
    let mut ui = Ui::new();
    let t = ui.add(Rect::new(10, 0, 100, 14), TextField::new("abcdef"));
    // 文字は x = 10 + 3 から 8 ピクセルずつ。'c' と 'd' の間（13 + 16 + 8 = 37 付近）を押す
    click(&mut ui, 13 + 3 * 8 - 2, 5, 0);
    if let Widget::TextField(f) = ui.widget_mut(t) {
        assert_eq!(f.cursor(), 3);
        f.max_len = 7;
    }
    ui.handle(typed('x'), 0);
    assert_eq!(ui.handle(typed('y'), 0), None, "full");
    assert_eq!(ui.text(t), "abcxdef");
}

#[test]
fn list_keyboard_selection_and_scrolling() {
    let mut ui = Ui::new();
    // 高さ 54 → 5 行見える
    let l = ui.add(Rect::new(0, 0, 100, 54), List::new(items(20)));
    assert_eq!(List::rows_in(ui.rect(l)), 5);
    assert_eq!(ui.handle(press(None, DOWN), 0), Some(Action::Selected(l, 0)));
    assert_eq!(ui.handle(press(None, DOWN), 0), Some(Action::Selected(l, 1)));
    assert_eq!(ui.handle(press(None, PAGE_DOWN), 0), Some(Action::Selected(l, 6)));
    // 選んだ項目が見えるように表示位置が動く
    assert_eq!(ui.list(l).top(), 2);
    assert_eq!(ui.handle(press(None, END), 0), Some(Action::Selected(l, 19)));
    assert_eq!(ui.list(l).top(), 15);
    // 末尾で ↓ は変わらない
    assert_eq!(ui.handle(press(None, DOWN), 0), None);
    assert_eq!(ui.handle(press(None, HOME), 0), Some(Action::Selected(l, 0)));
    assert_eq!(ui.list(l).top(), 0);
    assert_eq!(ui.handle(press(Some('\n'), ENTER), 0), Some(Action::Activated(l, 0)));
    assert_eq!(ui.list(l).selected_text(), Some("item0"));
}

#[test]
fn list_click_and_double_click() {
    let mut ui = Ui::new();
    let l = ui.add(Rect::new(0, 0, 100, 54), List::new(items(3)));
    // 2 行目（y = 2 + 10 .. 22）
    assert_eq!(click(&mut ui, 20, 15, 100), vec![Action::Selected(l, 1)]);
    assert_eq!(click(&mut ui, 20, 15, 120), vec![Action::Activated(l, 1)]);
    // 間が空いたら 2 回のクリック
    assert_eq!(click(&mut ui, 20, 15, 200), vec![Action::Selected(l, 1)]);
    assert_eq!(click(&mut ui, 20, 15, 200 + DOUBLE_CLICK_TICKS + 1), vec![Action::Selected(l, 1)]);
    // 違う項目なら 2 回目もただの選択
    assert_eq!(click(&mut ui, 20, 5, 400), vec![Action::Selected(l, 0)]);
    assert_eq!(click(&mut ui, 20, 25, 410), vec![Action::Selected(l, 2)]);
    // 項目の無い行
    assert!(click(&mut ui, 20, 45, 500).is_empty());
}

#[test]
fn list_scrollbar() {
    let mut ui = Ui::new();
    let l = ui.add(Rect::new(0, 0, 100, 54), List::new(items(20)));
    // 溝は x = 88..98, y = 2..52。つまみは先頭で高さ 50 * 5 / 20 = 12
    // つまみより下の溝を押すと 1 画面進む
    assert!(click(&mut ui, 92, 40, 0).is_empty());
    assert_eq!(ui.list(l).top(), 5);
    // つまみより上を押すと戻る
    click(&mut ui, 92, 3, 0);
    assert_eq!(ui.list(l).top(), 0);
    // つまみをいちばん下までドラッグする
    ui.handle(mouse(92, 5, 1), 0);
    ui.handle(mouse(92, 60, 1), 0);
    ui.handle(mouse(92, 60, 0), 0);
    assert_eq!(ui.list(l).top(), 15);
    // 項目の上のクリックはスクロールバーの左側だけ
    assert_eq!(click(&mut ui, 20, 5, 0), vec![Action::Selected(l, 15)]);
}

#[test]
fn list_set_items_resets_selection() {
    let mut ui = Ui::new();
    let l = ui.add(Rect::new(0, 0, 100, 54), List::new(items(20)));
    ui.list_mut(l).select(12);
    assert_eq!(ui.list(l).top(), 8);
    ui.list_mut(l).set_items(items(2));
    assert_eq!(ui.list(l).selected(), Some(0));
    assert_eq!(ui.list(l).top(), 0);
    ui.list_mut(l).set_items(Vec::new());
    assert_eq!(ui.list(l).selected(), None);
    assert!(matches!(ui.handle(press(None, DOWN), 0), Some(Action::Key(_))));
}

#[test]
fn checkbox_toggles() {
    let mut ui = Ui::new();
    let c = ui.add(Rect::new(0, 0, 80, 12), Checkbox::new("Sound", false));
    assert_eq!(click(&mut ui, 3, 5, 0), vec![Action::Toggled(c, true)]);
    assert!(ui.checked(c));
    assert_eq!(ui.handle(press(Some(' '), SPACE), 0), Some(Action::Toggled(c, false)));
    assert!(!ui.checked(c));
}

#[test]
fn dialog_buttons_and_keys() {
    let mut d = Dialog::yes_no(160, "Delete this file?");
    let (w, h) = d.size();
    assert_eq!(w, 160);
    // 文 1 行 + ボタン
    assert_eq!(h, 8 + 10 + 8 + 16 + 8);
    assert_eq!(d.handle(press(Some('\n'), ENTER), 0), Some(0));
    assert_eq!(d.handle(press(None, ESCAPE), 0), Some(1));
    // "No" のボタンを押す
    let no = d.ui.rect(2);
    assert_eq!(d.ui.text(2), "No");
    d.handle(mouse(no.x + 2, no.y + 2, 1), 0);
    assert_eq!(d.handle(mouse(no.x + 2, no.y + 2, 0), 0), Some(1));
}

#[test]
fn wrap_text() {
    assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(wrap("a\nb c", 10), ["a", "b c"]);
    assert_eq!(wrap("", 5), [""]);
}

#[test]
fn draws_widgets() {
    let mut buf = vec![FACE; 200 * 100];
    let mut s = Surface::new(&mut buf, 200, 100);
    let mut ui = Ui::new();
    let b = ui.add(Rect::new(2, 2, 40, 16), Button::new("OK"));
    let t = ui.add(Rect::new(50, 2, 60, 14), TextField::new("hi"));
    let l = ui.add(Rect::new(2, 30, 100, 54), List::new(items(20)));
    let d = ui.add(Rect::new(120, 2, 70, 16), Label::new("123").align(Align::Right).display());
    ui.list_mut(l).select(0);
    ui.set_focus(l);
    ui.draw(&mut s);
    let px = |s: &Surface, x, y| s.get(x, y).unwrap();
    // ボタンの地・テキスト欄の地・選んだ行（フォーカスがあるので青）・表示欄の地
    assert_eq!(px(&s, 5, 15), FACE);
    assert_eq!(px(&s, 52, 4), FIELD);
    assert_eq!(px(&s, 60, 33), SELECT);
    assert_eq!(px(&s, 122, 4), FIELD);
    // 文字がどこかに描かれている
    let r = ui.rect(d);
    let dark = (r.x..r.x + r.w).flat_map(|x| (r.y..r.y + r.h).map(move |y| (x, y))).any(|(x, y)| px(&s, x, y) == TEXT);
    assert!(dark, "label text");
    // 一覧の 2 行目は選んでいないので白地
    assert_eq!(px(&s, 60, 32 + ROW_H + 1), FIELD);
    let _ = (b, t);
}
