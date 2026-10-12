//! ダイアログ（文と、並んだボタン）
//!
//! アプリは小さなウィンドウを開き、`Dialog` に入力を渡して、押されたボタンの番号を受け取る。
//! Enter は最初のボタン（既定）、Esc は最後のボタン（取り消し）。

use alloc::string::String;
use alloc::vec::Vec;

use vibe_abi::key;
use vibe_gfx::Surface;

use crate::draw::{CHAR_W, FACE, LINE_H};
use crate::{Action, Align, Button, Id, Input, Label, Rect, Ui};

const MARGIN: i32 = 8;
const BUTTON_H: i32 = 16;
const BUTTON_MIN_W: i32 = 56;

pub struct Dialog {
    pub ui: Ui,
    buttons: Vec<Id>,
    w: usize,
    h: usize,
}

/// `text` を幅 `cols` 文字で折り返す（単語の切れ目で。長すぎる単語は切る）。`\n` で改行
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word;
            while word.len() > cols {
                if !line.is_empty() {
                    lines.push(core::mem::take(&mut line));
                }
                lines.push(String::from(&word[..cols]));
                word = &word[cols..];
            }
            if !line.is_empty() && line.len() + 1 + word.len() > cols {
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
        let wi = w as i32;
        let lines = wrap(text, ((wi - 2 * MARGIN) / CHAR_W) as usize);
        let text_h = lines.len() as i32 * LINE_H;
        let h = MARGIN + text_h + MARGIN + BUTTON_H + MARGIN;
        let mut ui = Ui::new();
        let label = Label::new(&lines.join("\n")).align(Align::Center);
        ui.add(Rect::new(MARGIN, MARGIN, wi - 2 * MARGIN, text_h), label);
        let n = buttons.len().max(1) as i32;
        let bw = ((wi - 2 * MARGIN - (n - 1) * MARGIN) / n).min(BUTTON_MIN_W.max(
            buttons.iter().map(|b| b.len() as i32 * CHAR_W + 16).max().unwrap_or(0),
        ));
        let total = n * bw + (n - 1) * MARGIN;
        let mut x = (wi - total) / 2;
        let y = h - MARGIN - BUTTON_H;
        let mut ids = Vec::new();
        for b in buttons {
            ids.push(ui.add(Rect::new(x, y, bw, BUTTON_H), Button::new(b)));
            x += bw + MARGIN;
        }
        Dialog { ui, buttons: ids, w, h: h as usize }
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
            Action::Key(k) if k.pressed() && k.key() == key::ENTER => Some(0),
            Action::Key(k) if k.pressed() && k.key() == key::ESCAPE => Some(self.buttons.len().saturating_sub(1)),
            _ => None,
        }
    }

    pub fn draw(&self, s: &mut Surface) {
        s.clear(FACE);
        self.ui.draw(s);
    }
}
