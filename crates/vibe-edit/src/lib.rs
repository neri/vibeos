//! テキストエディタの中身
//!
//! 行の並びとカーソル・表示範囲を持ち、操作（`Action`）を受けて書き換える。画面に出す内容は
//! `visible_rows` と `cursor_on_screen` で取り出す。キー入力やファイルの読み書きは呼び出し側が行う。
//!
//! 文字は ASCII だけを扱う（読み込み時に ASCII 以外は '?'、タブは空白 4 つ、CR は捨てる）。
//! そのため文字の位置とバイトの位置が同じになる。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Insert(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// ファイルの先頭・末尾
    Top,
    Bottom,
    /// 今の行を切り取る（続けて切り取ると貯まる）
    CutLine,
    /// 切り取った行を今の行の前に貼り付ける
    Paste,
}

pub struct Editor {
    lines: Vec<String>,
    row: usize,
    col: usize,
    /// 上下に動いたときに戻りたい桁
    want_col: usize,
    top: usize,
    left: usize,
    width: usize,
    height: usize,
    modified: bool,
    clipboard: Vec<String>,
    last_was_cut: bool,
}

/// 読み込んだ文字列を、エディタで扱える行に分ける
fn to_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| {
            let mut s = String::new();
            for c in l.chars() {
                match c {
                    '\r' => {}
                    '\t' => s.push_str(&" ".repeat(TAB_WIDTH - s.len() % TAB_WIDTH)),
                    c if c.is_ascii() && !c.is_ascii_control() => s.push(c),
                    _ => s.push('?'),
                }
            }
            s
        })
        .collect();
    // 最後の改行の後ろの空の行は行として数えない
    if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

impl Editor {
    /// `width` × `height` は文字を表示する範囲の大きさ
    pub fn new(text: &str, width: usize, height: usize) -> Editor {
        Editor {
            lines: to_lines(text),
            row: 0,
            col: 0,
            want_col: 0,
            top: 0,
            left: 0,
            width: width.max(1),
            height: height.max(1),
            modified: false,
            clipboard: Vec::new(),
            last_was_cut: false,
        }
    }

    /// 保存する内容（行の終わりはすべて改行。空のファイルなら空）
    pub fn text(&self) -> String {
        if self.lines.len() == 1 && self.lines[0].is_empty() {
            return String::new();
        }
        let mut s = String::new();
        for l in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    pub fn mark_saved(&mut self) {
        self.modified = false;
    }

    /// (行, 桁)。どちらも 0 始まり
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> &str {
        &self.lines[i]
    }

    fn line_len(&self) -> usize {
        self.lines[self.row].len()
    }

    /// 切り取った行を文字列で（行は改行でつなぐ）。システムのクリップボードに渡す
    pub fn clipboard_text(&self) -> String {
        self.clipboard.join("\n")
    }

    /// 貼り付ける行を文字列から決める（システムのクリップボードから。行に分けて読み込みと同じように直す）
    pub fn set_clipboard_text(&mut self, text: &str) {
        self.clipboard = if text.is_empty() { Vec::new() } else { to_lines(text) };
        self.last_was_cut = false;
    }

    pub fn apply(&mut self, action: Action) {
        let cut = action == Action::CutLine;
        match action {
            Action::Insert(c) => {
                if c == '\t' {
                    for _ in 0..TAB_WIDTH - self.col % TAB_WIDTH {
                        self.insert(' ');
                    }
                } else if c.is_ascii() && !c.is_ascii_control() {
                    self.insert(c);
                }
            }
            Action::Enter => {
                let rest = self.lines[self.row].split_off(self.col);
                self.lines.insert(self.row + 1, rest);
                self.row += 1;
                self.col = 0;
                self.modified = true;
            }
            Action::Backspace => {
                if self.col > 0 {
                    self.col -= 1;
                    self.lines[self.row].remove(self.col);
                    self.modified = true;
                } else if self.row > 0 {
                    let line = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.line_len();
                    self.lines[self.row].push_str(&line);
                    self.modified = true;
                }
            }
            Action::Delete => {
                if self.col < self.line_len() {
                    self.lines[self.row].remove(self.col);
                    self.modified = true;
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                    self.modified = true;
                }
            }
            Action::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len();
                }
            }
            Action::Right => {
                if self.col < self.line_len() {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            Action::Up => self.move_rows(-1),
            Action::Down => self.move_rows(1),
            Action::PageUp => self.move_rows(-(self.height as isize - 1).max(1)),
            Action::PageDown => self.move_rows((self.height as isize - 1).max(1)),
            Action::Home => self.col = 0,
            Action::End => self.col = self.line_len(),
            Action::Top => {
                self.row = 0;
                self.col = 0;
            }
            Action::Bottom => {
                self.row = self.lines.len() - 1;
                self.col = self.line_len();
            }
            Action::CutLine => {
                if !self.last_was_cut {
                    self.clipboard.clear();
                }
                if self.lines.len() == 1 {
                    let l = core::mem::take(&mut self.lines[0]);
                    self.clipboard.push(l);
                } else {
                    let l = self.lines.remove(self.row);
                    self.clipboard.push(l);
                    if self.row >= self.lines.len() {
                        self.row = self.lines.len() - 1;
                    }
                }
                self.col = 0;
                self.modified = true;
            }
            Action::Paste => {
                for (i, l) in self.clipboard.clone().into_iter().enumerate() {
                    self.lines.insert(self.row + i, l);
                }
                self.row += self.clipboard.len();
                self.col = 0;
                if !self.clipboard.is_empty() {
                    self.modified = true;
                }
            }
        }
        if !matches!(
            action,
            Action::Up | Action::Down | Action::PageUp | Action::PageDown
        ) {
            self.want_col = self.col;
        }
        self.last_was_cut = cut;
        self.scroll();
    }

    fn insert(&mut self, c: char) {
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
        self.modified = true;
    }

    fn move_rows(&mut self, delta: isize) {
        let last = self.lines.len() as isize - 1;
        self.row = (self.row as isize + delta).clamp(0, last) as usize;
        self.col = self.want_col.min(self.line_len());
    }

    /// カーソルが表示範囲に入るよう、表示の始まりを動かす
    fn scroll(&mut self) {
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + self.height {
            self.top = self.row + 1 - self.height;
        }
        // 横は、カーソルがはみ出したら幅の半分ずつ動かす（少しずつ動くと読みにくい）
        let step = (self.width / 2).max(1);
        while self.col < self.left {
            self.left = self.left.saturating_sub(step);
        }
        while self.col >= self.left + self.width {
            self.left += step;
        }
    }

    /// `query` を、カーソルの次の位置から後ろへ探す（最後まで行ったら先頭へ戻る）。見つかればそこへ動く
    pub fn find(&mut self, query: &str) -> bool {
        if query.is_empty() {
            return false;
        }
        let q = query.to_ascii_lowercase();
        let n = self.lines.len();
        for i in 0..=n {
            let r = (self.row + i) % n;
            let line = self.lines[r].to_ascii_lowercase();
            let from = if i == 0 { (self.col + 1).min(line.len()) } else { 0 };
            let hit = if i == n {
                // 一周して今の行の前半
                line[..self.col.min(line.len())].find(&q)
            } else {
                line[from..].find(&q).map(|p| p + from)
            };
            if let Some(c) = hit {
                self.row = r;
                self.col = c;
                self.want_col = c;
                self.last_was_cut = false;
                self.scroll();
                return true;
            }
        }
        false
    }

    /// 1 始まりの行番号へ動く
    pub fn goto_line(&mut self, line: usize) {
        self.row = line.clamp(1, self.lines.len()) - 1;
        self.col = 0;
        self.want_col = 0;
        self.scroll();
    }

    /// 表示する各行（幅に切った文字列。行が無いところは空）
    pub fn visible_rows(&self) -> Vec<String> {
        let mut out = vec![String::new(); self.height];
        for (i, row) in out.iter_mut().enumerate() {
            if let Some(l) = self.lines.get(self.top + i) {
                let start = self.left.min(l.len());
                let end = (self.left + self.width).min(l.len());
                row.push_str(&l[start..end]);
            }
        }
        out
    }

    /// カーソルの表示範囲の中での (行, 桁)
    pub fn cursor_on_screen(&self) -> (usize, usize) {
        (self.row - self.top, self.col - self.left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    fn ed(text: &str) -> Editor {
        Editor::new(text, 10, 3)
    }

    fn apply_all(e: &mut Editor, actions: &[Action]) {
        for &a in actions {
            e.apply(a);
        }
    }

    fn typ(e: &mut Editor, s: &str) {
        for c in s.chars() {
            e.apply(if c == '\n' { Enter } else { Insert(c) });
        }
    }

    #[test]
    fn load_and_save_round_trip() {
        assert_eq!(ed("a\nbc\n").text(), "a\nbc\n");
        assert_eq!(ed("a\nbc").text(), "a\nbc\n"); // 最後に改行を足す
        assert_eq!(ed("").text(), "");
        assert_eq!(ed("x\r\ny\r\n").text(), "x\ny\n");
        assert_eq!(ed("\tx\n").text(), "    x\n");
        assert_eq!(ed("caf\u{e9}\n").text(), "caf?\n");
        assert_eq!(ed("\n\n").line_count(), 2);
    }

    #[test]
    fn typing_enter_and_backspace() {
        let mut e = ed("");
        typ(&mut e, "hello\nworld");
        assert_eq!(e.text(), "hello\nworld\n");
        assert!(e.is_modified());
        apply_all(&mut e, &[Home, Backspace]); // 行の先頭で BackSpace = 前の行とつなぐ
        assert_eq!(e.text(), "helloworld\n");
        assert_eq!(e.cursor(), (0, 5));
        apply_all(&mut e, &[Enter]);
        assert_eq!(e.text(), "hello\nworld\n");
    }

    #[test]
    fn delete_joins_lines() {
        let mut e = ed("ab\ncd\n");
        apply_all(&mut e, &[End, Delete]);
        assert_eq!(e.text(), "abcd\n");
        apply_all(&mut e, &[Home, Delete]);
        assert_eq!(e.text(), "bcd\n");
    }

    #[test]
    fn cursor_moves_wrap_and_remember_column() {
        let mut e = ed("long line\nab\nanother\n");
        apply_all(&mut e, &[End]);
        assert_eq!(e.cursor(), (0, 9));
        apply_all(&mut e, &[Down]);
        assert_eq!(e.cursor(), (1, 2)); // 短い行では行末
        apply_all(&mut e, &[Down]);
        assert_eq!(e.cursor(), (2, 7)); // 戻りたい桁（9）に近づく
        apply_all(&mut e, &[Right, Right]);
        assert_eq!(e.cursor(), (2, 7)); // 最後の行の末尾から先へは行かない
        apply_all(&mut e, &[Top, Left]);
        assert_eq!(e.cursor(), (0, 0));
        apply_all(&mut e, &[Right; 9]);
        apply_all(&mut e, &[Right]);
        assert_eq!(e.cursor(), (1, 0)); // 行末から次の行の先頭へ
        apply_all(&mut e, &[Left]);
        assert_eq!(e.cursor(), (0, 9));
    }

    #[test]
    fn cut_and_paste_lines() {
        let mut e = ed("1\n2\n3\n4\n");
        apply_all(&mut e, &[Down, CutLine, CutLine]); // 続けて切ると貯まる
        assert_eq!(e.text(), "1\n4\n");
        assert_eq!(e.cursor(), (1, 0));
        apply_all(&mut e, &[Paste]); // 今の行（4）の前に貼る
        assert_eq!(e.text(), "1\n2\n3\n4\n");
        assert_eq!(e.cursor(), (3, 0));
        apply_all(&mut e, &[Top, CutLine, Bottom, Paste]); // 間に別の操作があれば貯め直す
        assert_eq!(e.text(), "2\n3\n1\n4\n");
        let mut e = ed("only\n");
        apply_all(&mut e, &[CutLine]);
        assert_eq!(e.text(), "");
        apply_all(&mut e, &[Paste]);
        assert_eq!(e.text(), "only\n\n");
    }

    #[test]
    fn scrolling_keeps_cursor_visible() {
        let mut e = ed("0\n1\n2\n3\n4\n5\n");
        apply_all(&mut e, &[Down, Down, Down]);
        assert_eq!(e.visible_rows(), ["1", "2", "3"]);
        assert_eq!(e.cursor_on_screen(), (2, 0));
        apply_all(&mut e, &[Bottom]);
        assert_eq!(e.visible_rows(), ["3", "4", "5"]);
        apply_all(&mut e, &[PageUp]); // 2 行上へ（Bottom で行末に行ったので桁は 1）
        assert_eq!(e.cursor(), (3, 1));
        apply_all(&mut e, &[Top]);
        assert_eq!(e.visible_rows(), ["0", "1", "2"]);
        // 横: 幅 10 を超えると半分（5）ずつずらす
        let mut e = ed("abcdefghijklmnopqrstuvwxyz\nshort\n");
        apply_all(&mut e, &[End]);
        assert_eq!(e.visible_rows()[0], "uvwxyz");
        assert_eq!(e.cursor_on_screen(), (0, 6));
        assert_eq!(e.visible_rows()[1], "");
        apply_all(&mut e, &[Home]);
        assert_eq!(e.visible_rows()[0], "abcdefghij");
    }

    #[test]
    fn find_wraps_and_ignores_case() {
        let mut e = ed("alpha\nbeta gamma\nGamma ray\n");
        assert!(e.find("gamma"));
        assert_eq!(e.cursor(), (1, 5));
        assert!(e.find("gamma"));
        assert_eq!(e.cursor(), (2, 0));
        assert!(e.find("gamma"));
        assert_eq!(e.cursor(), (1, 5)); // 先頭へ戻って見つける
        assert!(!e.find("zeta"));
        assert_eq!(e.cursor(), (1, 5));
    }

    #[test]
    fn tab_inserts_spaces_to_next_stop() {
        let mut e = ed("");
        typ(&mut e, "ab\tc");
        assert_eq!(e.text(), "ab  c\n");
    }

    #[test]
    fn goto_line_clamps() {
        let mut e = ed("a\nb\nc\n");
        e.goto_line(2);
        assert_eq!(e.cursor(), (1, 0));
        e.goto_line(99);
        assert_eq!(e.cursor(), (2, 0));
        e.goto_line(0);
        assert_eq!(e.cursor(), (0, 0));
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn clipboard_as_text() {
        let mut e = Editor::new("one\ntwo\nthree\n", 40, 10);
        e.apply(Action::CutLine);
        e.apply(Action::CutLine);
        assert_eq!(e.clipboard_text(), "one\ntwo");
        // 外から来た文字列を貼り付ける（タブは空白に直る）
        e.set_clipboard_text("a\tb\nc");
        e.apply(Action::Paste);
        assert_eq!(e.text(), "a   b\nc\nthree\n");
        e.set_clipboard_text("");
        assert_eq!(e.clipboard_text(), "");
    }
}
