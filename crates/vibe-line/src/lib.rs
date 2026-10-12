//! 1 行入力の編集（左右移動・挿入・削除・履歴・Ctrl+V で貼り付け）
//!
//! キーを読むこと・文字を書くこと・クリップボードは `Term` に任せる（カーネルは端末のウィンドウの
//! キーと `kprint!`、ユーザ空間のシェルは `getkey` と標準出力）。画面への反映は端末の相対移動
//! （`\x08`, `ESC [ n C` / `D`）だけで行う。端末は左右移動で行をまたぐので、入力が画面の 1 行を
//! 超えても編集できる。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const HISTORY_MAX: usize = 32;
const LINE_MAX: usize = 256;

/// 編集に使うキー（押したときだけ渡す）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// 表示できる文字
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
    /// Ctrl+C: 取り消す
    Cancel,
    /// Ctrl+L: 画面を消して描き直す
    Redraw,
    /// Ctrl+V: クリップボードの 1 行目を入れる
    Paste,
    /// それ以外（無視する）
    Other,
}

impl Key {
    /// キーの種類と文字から（`left` などはカーソルキーなど文字の無いキー）。
    /// 文字の制御コードは Enter（`\n`）・BackSpace（`\x08`）・Ctrl+C・Ctrl+L・Ctrl+V を見る
    pub fn from_char(ch: Option<char>) -> Key {
        match ch {
            Some('\n') => Key::Enter,
            Some('\x08') => Key::Backspace,
            Some('\x03') => Key::Cancel,
            Some('\x0c') => Key::Redraw,
            Some('\x16') => Key::Paste,
            Some(c) if !c.is_control() => Key::Char(c),
            _ => Key::Other,
        }
    }
}

/// 端末（キーを読む・書く）
pub trait Term {
    /// 次に押されたキー。None は「もう読めない」（端末が閉じた・止めるよう頼まれた）
    fn read_key(&mut self) -> Option<Key>;
    fn write(&mut self, s: &str);
    fn clipboard(&mut self) -> String;
}

/// 入力の結果
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Line(String),
    /// Ctrl+C で取り消した（端末が閉じたときも）
    Cancelled,
}

#[derive(Clone, Default)]
pub struct LineEditor {
    history: VecDeque<String>,
}

struct State<'a, T: Term> {
    term: &'a mut T,
    buf: Vec<char>,
    cursor: usize,
}

impl<T: Term> State<'_, T> {
    fn back(&mut self, n: usize) {
        if n > 0 {
            self.term.write(&format!("\x1b[{n}D"));
        }
    }

    fn forward(&mut self, n: usize) {
        if n > 0 {
            self.term.write(&format!("\x1b[{n}C"));
        }
    }

    fn print_from(&mut self, from: usize) {
        let s: String = self.buf[from..].iter().collect();
        self.term.write(&s);
    }

    fn insert(&mut self, c: char) {
        if self.buf.len() >= LINE_MAX {
            return;
        }
        self.buf.insert(self.cursor, c);
        self.print_from(self.cursor);
        self.cursor += 1;
        self.back(self.buf.len() - self.cursor);
    }

    /// カーソル位置の文字を消して後ろを詰める
    fn delete_at_cursor(&mut self) {
        self.buf.remove(self.cursor);
        self.print_from(self.cursor);
        self.term.write(" ");
        self.back(self.buf.len() - self.cursor + 1);
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.back(1);
            self.delete_at_cursor();
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.delete_at_cursor();
        }
    }

    fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.back(1);
        }
    }

    fn right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
            self.forward(1);
        }
    }

    fn home(&mut self) {
        self.back(self.cursor);
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.forward(self.buf.len() - self.cursor);
        self.cursor = self.buf.len();
    }

    /// 入力行全体を置き換える（履歴の呼び出し）
    fn replace(&mut self, new: &str) {
        let old_len = self.buf.len();
        self.home();
        self.buf = new.chars().collect();
        self.print_from(0);
        let new_len = self.buf.len();
        if new_len < old_len {
            for _ in new_len..old_len {
                self.term.write(" ");
            }
            self.back(old_len - new_len);
        }
        self.cursor = new_len;
    }
}

impl LineEditor {
    pub const fn new() -> Self {
        LineEditor { history: VecDeque::new() }
    }

    pub fn history(&self) -> impl Iterator<Item = &String> {
        self.history.iter()
    }

    /// プロンプトを出して 1 行読む
    pub fn read_line(&mut self, term: &mut impl Term, prompt: &str) -> Input {
        term.write(prompt);
        let mut st = State { term, buf: Vec::new(), cursor: 0 };
        // 履歴をたどっている位置（history.len() は「新しく打っている行」）
        let mut hist_pos = self.history.len();
        let mut draft = String::new();
        loop {
            let Some(key) = st.term.read_key() else {
                return Input::Cancelled;
            };
            match key {
                Key::Left => st.left(),
                Key::Right => st.right(),
                Key::Home => st.home(),
                Key::End => st.end(),
                Key::Delete => st.delete(),
                Key::Backspace => st.backspace(),
                Key::Up => {
                    if hist_pos > 0 {
                        if hist_pos == self.history.len() {
                            draft = st.buf.iter().collect();
                        }
                        hist_pos -= 1;
                        st.replace(&self.history[hist_pos].clone());
                    }
                }
                Key::Down => {
                    if hist_pos < self.history.len() {
                        hist_pos += 1;
                        let line = if hist_pos == self.history.len() { draft.clone() } else { self.history[hist_pos].clone() };
                        st.replace(&line);
                    }
                }
                Key::Enter => {
                    st.end();
                    st.term.write("\n");
                    let line: String = st.buf.iter().collect();
                    if !line.trim().is_empty() && self.history.back() != Some(&line) {
                        if self.history.len() == HISTORY_MAX {
                            self.history.pop_front();
                        }
                        self.history.push_back(line.clone());
                    }
                    return Input::Line(line);
                }
                Key::Paste => {
                    let clip = st.term.clipboard();
                    for c in clip.chars().take_while(|&c| c != '\n') {
                        if !c.is_control() {
                            st.insert(c);
                        }
                    }
                }
                Key::Cancel => {
                    st.end();
                    st.term.write("^C\n");
                    return Input::Cancelled;
                }
                Key::Redraw => {
                    // 画面を消して入力中の行を描き直す
                    st.term.write(&format!("\x1b[2J\x1b[H{prompt}"));
                    st.print_from(0);
                    let n = st.buf.len() - st.cursor;
                    st.back(n);
                }
                Key::Char(c) => st.insert(c),
                Key::Other => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
