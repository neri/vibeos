//! 1 行入力の編集（中身は `crates/vibe-line`）を、今のタスクの端末につなぐ
//!
//! キーは端末のウィンドウから（`gui::read_key`）、表示は `kprint!`、貼り付けはクリップボードから。
//! 予備のシェルと、端末の fd 0 を読むとき（`io::read`）に使う。

use alloc::string::String;

use vibe_keyboard::KeyCode;
use vibe_line::{Key, Term};
pub use vibe_line::Input;

use crate::gui;

struct KernelTerm;

impl Term for KernelTerm {
    fn read_key(&mut self) -> Option<Key> {
        loop {
            let ev = gui::read_key(crate::task::sched::current_tty())?;
            if !ev.pressed {
                continue;
            }
            return Some(match ev.key {
                KeyCode::LEFT => Key::Left,
                KeyCode::RIGHT => Key::Right,
                KeyCode::HOME => Key::Home,
                KeyCode::END => Key::End,
                KeyCode::DELETE => Key::Delete,
                KeyCode::UP => Key::Up,
                KeyCode::DOWN => Key::Down,
                _ => Key::from_char(ev.ch),
            });
        }
    }

    fn write(&mut self, s: &str) {
        kprint!("{s}");
    }

    fn clipboard(&mut self) -> String {
        crate::clipboard::get()
    }
}

#[derive(Clone)]
pub struct LineEditor(vibe_line::LineEditor);

impl LineEditor {
    pub const fn new() -> Self {
        LineEditor(vibe_line::LineEditor::new())
    }

    /// プロンプトを出して 1 行読む
    pub fn read_line(&mut self, prompt: &str) -> Input {
        self.0.read_line(&mut KernelTerm, prompt)
    }
}
