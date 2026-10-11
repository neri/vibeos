//! テキストエディタ（コンソールの 40 桁 × 24 行を全部使う）
//!
//! 1 行目: ファイル名など、2〜23 行目: 本文、24 行目: キーの説明・メッセージ。
//! Ctrl+S 保存（フロッピーへの書き戻しもする）, Ctrl+Q 終わる, Ctrl+F 探す, Ctrl+G 行へ移動,
//! Ctrl+K 行を切り取る, Ctrl+U 貼り付ける, Ctrl+Home / Ctrl+End ファイルの先頭・末尾。
//! Ctrl+C はカーネルがプログラムを止める（保存されない）。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use vibe_edit::{Action, Editor};
use vibe_rt::abi::{self, KeyEvent, err, key};
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

const COLS: usize = 40;
const ROWS: usize = 24;
const TEXT_ROWS: usize = ROWS - 2;
const HELP: &str = "^S Save ^Q Quit ^F Find ^K Cut ^U Paste";

struct App {
    path: String,
    editor: Editor,
    is_new: bool,
    message: Option<String>,
    quit_armed: bool,
}

fn main() -> i32 {
    let Some(path) = vibe_rt::args().nth(1) else {
        println!("usage: edit file");
        return 2;
    };
    let (text, is_new) = match vibe_rt::read_file(path) {
        Ok(data) => (String::from_utf8_lossy(&data).into_owned(), false),
        Err(err::ENOENT) => (String::new(), true),
        Err(e) => {
            println!("edit: {path}: {}", err::name(e));
            return 1;
        }
    };
    let mut app = App {
        path: String::from(path),
        editor: Editor::new(&text, COLS, TEXT_ROWS),
        is_new,
        message: None,
        quit_armed: false,
    };
    loop {
        app.render();
        let k = vibe_rt::wait_key();
        if !app.handle(k) {
            break;
        }
    }
    // 画面を消してシェルに戻る
    vibe_rt::print!("\x1b[0m\x1b[2J\x1b[H");
    0
}

/// `s` を幅 `w` に切るか空白で埋める
fn fit(s: &str, w: usize) -> String {
    let mut out: String = s.chars().take(w).collect();
    while out.len() < w {
        out.push(' ');
    }
    out
}

impl App {
    fn render(&self) {
        let e = &self.editor;
        let mut out = String::new();
        // 1 行目: ファイル名と位置
        let (row, col) = e.cursor();
        let name = if self.is_new { format!("{} (new)", self.path) } else { self.path.clone() };
        let flag = if e.is_modified() { "*" } else { " " };
        let pos = format!("{}:{}", row + 1, col + 1);
        let title = format!("{}{flag}", fit(&format!(" {name}"), COLS - 1 - pos.len() - 1));
        out.push_str(&format!("\x1b[1;1H\x1b[30;47m{}{pos} \x1b[0m", title));
        // 本文
        for (i, line) in e.visible_rows().iter().enumerate() {
            out.push_str(&format!("\x1b[{};1H{line}\x1b[K", i + 2));
        }
        // 最後の行: メッセージかキーの説明
        let status = self.message.as_deref().unwrap_or(HELP);
        out.push_str(&format!("\x1b[{ROWS};1H\x1b[30;47m{}\x1b[0m", fit(status, COLS)));
        let (r, c) = e.cursor_on_screen();
        out.push_str(&format!("\x1b[{};{}H", r + 2, c + 1));
        sys::write(abi::STDOUT, out.as_bytes());
    }

    /// キーを 1 つ処理する。終わるなら false
    fn handle(&mut self, k: KeyEvent) -> bool {
        // 修飾キー（Shift・Ctrl・Alt・CapsLock）だけを押したときは何もしない（メッセージなども消さない）
        if matches!(k.key() & 0x7F, 0x1D | 0x2A | 0x36 | 0x38 | 0x3A) {
            return true;
        }
        self.message = None;
        let quit_armed = core::mem::take(&mut self.quit_armed);
        let action = match (k.key(), k.ch()) {
            (_, Some('\x13')) => {
                self.save();
                return true;
            }
            (_, Some('\x11')) => {
                if self.editor.is_modified() && !quit_armed {
                    self.message = Some("Unsaved changes! ^Q again to quit".into());
                    self.quit_armed = true;
                    return true;
                }
                return false;
            }
            (_, Some('\x06')) => {
                if let Some(q) = self.prompt("Find: ") {
                    if !self.editor.find(&q) {
                        self.message = Some(format!("Not found: {q}"));
                    } else {
                        self.message = Some(format!("Found at line {}", self.editor.cursor().0 + 1));
                    }
                }
                return true;
            }
            (_, Some('\x07')) => {
                if let Some(n) = self.prompt("Go to line: ").and_then(|s| s.trim().parse().ok()) {
                    self.editor.goto_line(n);
                }
                return true;
            }
            (_, Some('\x0b')) => Action::CutLine,
            (_, Some('\x15')) => Action::Paste,
            (key::UP, _) => Action::Up,
            (key::DOWN, _) => Action::Down,
            (key::LEFT, _) => Action::Left,
            (key::RIGHT, _) => Action::Right,
            (key::HOME, _) if k.ctrl() => Action::Top,
            (key::END, _) if k.ctrl() => Action::Bottom,
            (key::HOME, _) => Action::Home,
            (key::END, _) => Action::End,
            (key::PAGE_UP, _) => Action::PageUp,
            (key::PAGE_DOWN, _) => Action::PageDown,
            (key::DELETE, _) => Action::Delete,
            (key::BACKSPACE, _) => Action::Backspace,
            (key::ENTER, _) => Action::Enter,
            (key::TAB, _) => Action::Insert('\t'),
            (_, Some(c)) if !c.is_control() => Action::Insert(c),
            _ => return true,
        };
        self.editor.apply(action);
        true
    }

    fn save(&mut self) {
        let text = self.editor.text();
        let fd = sys::open_with(&self.path, abi::OPEN_WRITE | abi::OPEN_CREATE | abi::OPEN_TRUNCATE);
        if fd < 0 {
            self.message = Some(format!("Cannot save: {}", err::name(-fd)));
            return;
        }
        let n = sys::write(fd, text.as_bytes());
        sys::close(fd);
        if n < 0 || n as usize != text.len() {
            self.message = Some(format!("Write error: {}", err::name(-n)));
            return;
        }
        self.editor.mark_saved();
        self.is_new = false;
        // フロッピーにも書き戻す（ドライブが無ければメモリ上だけ）
        let synced = sys::sync();
        self.message = Some(if synced >= 0 {
            format!("Saved {} bytes", text.len())
        } else {
            format!("Saved {} bytes (memory only)", text.len())
        });
    }

    /// 最後の行で 1 行入力する。Esc で取りやめ
    fn prompt(&mut self, label: &str) -> Option<String> {
        let mut input = String::new();
        loop {
            let line = fit(&format!("{label}{input}"), COLS);
            let cursor = (label.len() + input.len()).min(COLS - 1) + 1;
            vibe_rt::print!("\x1b[{ROWS};1H\x1b[30;47m{line}\x1b[0m\x1b[{ROWS};{cursor}H");
            let k = vibe_rt::wait_key();
            match (k.key(), k.ch()) {
                (key::ESCAPE, _) => return None,
                (key::ENTER, _) => return Some(input),
                (key::BACKSPACE, _) => {
                    input.pop();
                }
                (_, Some(c)) if !c.is_control() && label.len() + input.len() < COLS - 1 => input.push(c),
                _ => {}
            }
        }
    }
}
