//! ダイアログ（文と、並んだボタン。`prompt` なら 1 行のテキスト欄も）
//!
//! アプリは小さなウィンドウを開き、`Dialog` に入力を渡して、押されたボタンの番号を受け取る。
//! Enter は最初のボタン（既定）、Esc は最後のボタン（取り消し）。テキスト欄の中身は `input` で取る。

use alloc::string::String;
use alloc::vec::Vec;

use vibe_abi::key;
use vibe_gfx::canvas::Canvas;

use crate::draw::{self, FACE};
use crate::{Action, Align, Button, Id, Input, Label, Rect, TextField, Ui};

pub const MARGIN: i32 = 12;
pub const BUTTON_H: i32 = 26;
const BUTTON_MIN_W: i32 = 72;
pub const FIELD_H: i32 = 26;

/// 文の 1 行の高さ（フォントの行の高さ + 2）
pub fn line_h() -> i32 {
    draw::text().height() + 2
}

pub struct Dialog {
    pub ui: Ui,
    buttons: Vec<Id>,
    field: Option<Id>,
    w: usize,
    h: usize,
}

/// `text` を幅 `cols` 文字で折り返す（単語の切れ目で。長すぎる単語は切る）。`\n` で改行
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1) as i32;
    wrap_by(text, cols, |s| s.len() as i32)
}

/// `text` を幅 `max` に折り返す（幅は `width` で測る）
pub fn wrap_by(text: &str, max: i32, width: impl Fn(&str) -> i32) -> Vec<String> {
    let max = max.max(1);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word;
            while width(word) > max {
                if !line.is_empty() {
                    lines.push(core::mem::take(&mut line));
                }
                // 入るだけ（少なくとも 1 文字）を切り出す
                let n = (1..=word.len()).take_while(|&n| width(&word[..n]) <= max).last().unwrap_or(1);
                lines.push(String::from(&word[..n]));
                word = &word[n..];
            }
            if !line.is_empty() && width(&line) + width(" ") + width(word) > max {
                lines.push(core::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

impl Dialog {
    /// 幅 `w` のダイアログ。高さは文の行数で決まる（`size` で取れる）
    pub fn new(w: usize, text: &str, buttons: &[&str]) -> Dialog {
        Self::build(w, text, None, buttons)
    }

    /// 文と 1 行のテキスト欄（中身は `initial`。選んだ状態で始まる）とボタン。テキスト欄の Enter は最初のボタン
    pub fn prompt(w: usize, text: &str, initial: &str, buttons: &[&str]) -> Dialog {
        Self::build(w, text, Some(initial), buttons)
    }

    fn build(w: usize, text: &str, field: Option<&str>, buttons: &[&str]) -> Dialog {
        let wi = w as i32;
        let t = draw::text();
        let lines = wrap_by(text, wi - 2 * MARGIN - 12, |s| t.width(s));
        let text_h = lines.len() as i32 * line_h();
        let field_h = if field.is_some() { FIELD_H + MARGIN } else { 0 };
        let h = MARGIN + text_h + MARGIN + field_h + BUTTON_H + MARGIN;
        let mut ui = Ui::new();
        let align = if field.is_some() { Align::Left } else { Align::Center };
        let label = Label::new(&lines.join("\n")).align(align);
        ui.add(Rect::new(MARGIN, MARGIN, wi - 2 * MARGIN, text_h), label);
        let field = field.map(|initial| {
            let r = Rect::new(MARGIN, MARGIN + text_h + MARGIN, wi - 2 * MARGIN, FIELD_H);
            // 最初の中身は選んだ状態にする（打てば置き換わる）
            let mut t = TextField::new(initial);
            t.select_all();
            ui.add(r, t)
        });
        let n = buttons.len().max(1) as i32;
        let bw = ((wi - 2 * MARGIN - (n - 1) * MARGIN) / n)
            .min(BUTTON_MIN_W.max(buttons.iter().map(|b| t.width(b) + 24).max().unwrap_or(0)));
        let total = n * bw + (n - 1) * MARGIN;
        let mut x = (wi - total) / 2;
        let y = h - MARGIN - BUTTON_H;
        let mut ids = Vec::new();
        // 最初のボタン（Enter で押されるもの）は既定のボタン
        for (i, b) in buttons.iter().enumerate() {
            let button = if i == 0 { Button::new(b).primary() } else { Button::new(b) };
            ids.push(ui.add(Rect::new(x, y, bw, BUTTON_H), button));
            x += bw + MARGIN;
        }
        if let Some(f) = field {
            ui.set_focus(f);
        }
        Dialog { ui, buttons: ids, field, w, h: h as usize }
    }

    /// テキスト欄の中身（`prompt` でなければ空）
    pub fn input(&self) -> &str {
        self.field.map_or("", |f| self.ui.text(f))
    }

    /// はい・いいえ
    pub fn yes_no(w: usize, text: &str) -> Dialog {
        Dialog::new(w, text, &["Yes", "No"])
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// 入力を渡す。ボタンが押されたらその番号
    pub fn handle(&mut self, input: Input, now: u32) -> Option<usize> {
        match self.ui.handle(input, now)? {
            Action::Clicked(id) => self.buttons.iter().position(|&b| b == id),
            Action::Submitted(_) => Some(0),
            Action::Key(k) if k.pressed() && k.key() == key::ENTER => Some(0),
            Action::Key(k) if k.pressed() && k.key() == key::ESCAPE => Some(self.buttons.len().saturating_sub(1)),
            _ => None,
        }
    }

    pub fn draw(&self, c: &mut Canvas) {
        c.clear(FACE);
        self.ui.draw(c);
    }
}
