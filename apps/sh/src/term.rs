//! 行の編集（vibe-line）を端末につなぐ: キーは getkey、出力は標準出力

use alloc::string::String;

use vibe_line::{Key, Term};
use vibe_rt::abi::{self, key};
use vibe_rt::{io, sys};

pub struct Terminal {
    /// キーがもう読めない（端末が閉じた・止めるよう頼まれた）
    pub closed: bool,
}

impl Term for Terminal {
    fn read_key(&mut self) -> Option<Key> {
        loop {
            let Some(k) = sys::getkey(false) else {
                self.closed = true;
                return None;
            };
            if !k.pressed() {
                continue;
            }
            return Some(match k.key() {
                key::LEFT => Key::Left,
                key::RIGHT => Key::Right,
                key::HOME => Key::Home,
                key::END => Key::End,
                key::DELETE => Key::Delete,
                key::UP => Key::Up,
                key::DOWN => Key::Down,
                _ => Key::from_char(k.ch()),
            });
        }
    }

    fn write(&mut self, s: &str) {
        let _ = io::write_all(abi::STDOUT, s.as_bytes());
    }

    fn clipboard(&mut self) -> String {
        sys::clip_get()
    }
}
