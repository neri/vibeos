//! 1 行入力の編集（左右移動・挿入・削除・履歴・Ctrl+V で貼り付け）
//!
//! 画面への反映はコンソールの相対移動（`\x08`, `ESC [ n C` / `D`）だけで行う。
//! コンソールは左右移動で行をまたぐので、入力が画面の 1 行を超えても編集できる。

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_keyboard::KeyCode;

use crate::gui;

const HISTORY_MAX: usize = 32;
const LINE_MAX: usize = 256;

pub struct LineEditor {
    history: VecDeque<String>,
}

/// 入力の結果
pub enum Input {
    Line(String),
    /// Ctrl+C で取り消した
    Cancelled,
}

struct State {
    buf: Vec<char>,
    cursor: usize,
}

fn back(n: usize) {
    if n > 0 {
        kprint!("\x1b[{n}D");
    }
}

fn forward(n: usize) {
    if n > 0 {
        kprint!("\x1b[{n}C");
    }
}

fn print_chars(chars: &[char]) {
    let s: String = chars.iter().collect();
    kprint!("{s}");
}

impl State {
    fn insert(&mut self, c: char) {
        if self.buf.len() >= LINE_MAX {
            return;
        }
        self.buf.insert(self.cursor, c);
        print_chars(&self.buf[self.cursor..]);
        self.cursor += 1;
        back(self.buf.len() - self.cursor);
    }

    /// カーソル位置の文字を消して後ろを詰める
    fn delete_at_cursor(&mut self) {
        self.buf.remove(self.cursor);
        print_chars(&self.buf[self.cursor..]);
        kprint!(" ");
        back(self.buf.len() - self.cursor + 1);
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            back(1);
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
            back(1);
        }
    }

    fn right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
            forward(1);
        }
    }

    fn home(&mut self) {
        back(self.cursor);
        self.cursor = 0;
    }

    fn end(&mut self) {
        forward(self.buf.len() - self.cursor);
        self.cursor = self.buf.len();
    }

    /// 入力行全体を置き換える（履歴の呼び出し）
    fn replace(&mut self, new: &str) {
        let old_len = self.buf.len();
        self.home();
        self.buf = new.chars().collect();
        print_chars(&self.buf);
        let new_len = self.buf.len();
        if new_len < old_len {
            for _ in new_len..old_len {
                kprint!(" ");
            }
            back(old_len - new_len);
        }
        self.cursor = new_len;
    }
}

impl LineEditor {
    pub const fn new() -> Self {
        LineEditor { history: VecDeque::new() }
    }

    #[allow(dead_code)]
    pub fn history(&self) -> impl Iterator<Item = &String> {
        self.history.iter()
    }

    /// プロンプトを出して 1 行読む
    pub fn read_line(&mut self, prompt: &str) -> Input {
        kprint!("{prompt}");
        let mut st = State { buf: Vec::new(), cursor: 0 };
        // 履歴をたどっている位置（history.len() は「新しく打っている行」）
        let mut hist_pos = self.history.len();
        let mut draft = String::new();
        loop {
            let Some(ev) = gui::read_key(crate::task::sched::current_tty()) else {
                return Input::Cancelled;
            };
            if !ev.pressed {
                continue;
            }
            match ev.key {
                KeyCode::LEFT => st.left(),
                KeyCode::RIGHT => st.right(),
                KeyCode::HOME => st.home(),
                KeyCode::END => st.end(),
                KeyCode::DELETE => st.delete(),
                KeyCode::UP => {
                    if hist_pos > 0 {
                        if hist_pos == self.history.len() {
                            draft = st.buf.iter().collect();
                        }
                        hist_pos -= 1;
                        st.replace(&self.history[hist_pos].clone());
                    }
                }
                KeyCode::DOWN => {
                    if hist_pos < self.history.len() {
                        hist_pos += 1;
                        let line = if hist_pos == self.history.len() { draft.clone() } else { self.history[hist_pos].clone() };
                        st.replace(&line);
                    }
                }
                _ => match ev.ch {
                    Some('\n') => {
                        st.end();
                        kprintln!();
                        let line: String = st.buf.iter().collect();
                        if !line.trim().is_empty() && self.history.back() != Some(&line) {
                            if self.history.len() == HISTORY_MAX {
                                self.history.pop_front();
                            }
                            self.history.push_back(line.clone());
                        }
                        return Input::Line(line);
                    }
                    Some('\x08') => st.backspace(),
                    Some('\x16') => {
                        // Ctrl+V: クリップボードの 1 行目を入れる
                        for c in crate::clipboard::get().chars().take_while(|&c| c != '\n') {
                            if !c.is_control() {
                                st.insert(c);
                            }
                        }
                    }
                    Some('\x03') => {
                        st.end();
                        kprintln!("^C");
                        return Input::Cancelled;
                    }
                    Some('\x0c') => {
                        // Ctrl+L: 画面を消して入力中の行を描き直す
                        kprint!("\x1b[2J\x1b[H{prompt}");
                        print_chars(&st.buf);
                        back(st.buf.len() - st.cursor);
                    }
                    Some(c) if !c.is_control() => st.insert(c),
                    _ => {}
                },
            }
        }
    }
}
