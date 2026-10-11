//! PS/2 キーボードのスキャンコード（セット 1）の解釈。
//!
//! 8042 コントローラの変換機能が有効なら、キーボードの内部のセットに関係なく
//! セット 1 で届く。ここではバイト列をキーイベントに変え、キー配列（`Layout`）で
//! 文字に変換する。キー配列は表の差し替えで追加できる（現在は US のみ）。

#![cfg_attr(not(test), no_std)]

mod us;

pub use us::{US, UsLayout};

/// 物理的なキー。値はセット 1 のメイクコード（E0 付きは 0xE0xx）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyCode(pub u16);

impl KeyCode {
    pub const ESCAPE: KeyCode = KeyCode(0x01);
    pub const BACKSPACE: KeyCode = KeyCode(0x0E);
    pub const TAB: KeyCode = KeyCode(0x0F);
    pub const ENTER: KeyCode = KeyCode(0x1C);
    pub const LEFT_CTRL: KeyCode = KeyCode(0x1D);
    pub const LEFT_SHIFT: KeyCode = KeyCode(0x2A);
    pub const RIGHT_SHIFT: KeyCode = KeyCode(0x36);
    pub const LEFT_ALT: KeyCode = KeyCode(0x38);
    pub const SPACE: KeyCode = KeyCode(0x39);
    pub const CAPS_LOCK: KeyCode = KeyCode(0x3A);
    pub const F1: KeyCode = KeyCode(0x3B);
    pub const F10: KeyCode = KeyCode(0x44);
    pub const NUM_LOCK: KeyCode = KeyCode(0x45);
    pub const SCROLL_LOCK: KeyCode = KeyCode(0x46);
    pub const F11: KeyCode = KeyCode(0x57);
    pub const F12: KeyCode = KeyCode(0x58);
    pub const KEYPAD_ENTER: KeyCode = KeyCode(0xE01C);
    pub const RIGHT_CTRL: KeyCode = KeyCode(0xE01D);
    pub const KEYPAD_SLASH: KeyCode = KeyCode(0xE035);
    pub const RIGHT_ALT: KeyCode = KeyCode(0xE038);
    pub const HOME: KeyCode = KeyCode(0xE047);
    pub const UP: KeyCode = KeyCode(0xE048);
    pub const PAGE_UP: KeyCode = KeyCode(0xE049);
    pub const LEFT: KeyCode = KeyCode(0xE04B);
    pub const RIGHT: KeyCode = KeyCode(0xE04D);
    pub const END: KeyCode = KeyCode(0xE04F);
    pub const DOWN: KeyCode = KeyCode(0xE050);
    pub const PAGE_DOWN: KeyCode = KeyCode(0xE051);
    pub const INSERT: KeyCode = KeyCode(0xE052);
    pub const DELETE: KeyCode = KeyCode(0xE053);

    /// F1〜F12 なら番号を返す
    pub fn function_number(self) -> Option<u8> {
        match self.0 {
            0x3B..=0x44 => Some((self.0 - 0x3B + 1) as u8),
            0x57 => Some(11),
            0x58 => Some(12),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub left_shift: bool,
    pub right_shift: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub caps_lock: bool,
}

impl Modifiers {
    pub fn shift(&self) -> bool {
        self.left_shift || self.right_shift
    }
    pub fn ctrl(&self) -> bool {
        self.left_ctrl || self.right_ctrl
    }
    pub fn alt(&self) -> bool {
        self.left_alt || self.right_alt
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: KeyCode,
    pub pressed: bool,
    /// このイベントを反映した後の修飾キーの状態
    pub mods: Modifiers,
    /// 押したときに入力される文字（離したとき・文字の無いキーは None）
    pub ch: Option<char>,
}

/// キー配列: キーと修飾キーから文字を決める
pub trait Layout {
    fn char_for(&self, key: KeyCode, mods: &Modifiers) -> Option<char>;
}

pub struct Decoder<L: Layout + 'static> {
    layout: &'static L,
    e0: bool,
    /// Pause キー（E1 で始まる 6 バイト）の残りを読み飛ばす数
    skip: u8,
    mods: Modifiers,
}

impl<L: Layout> Decoder<L> {
    pub const fn new(layout: &'static L) -> Self {
        Decoder { layout, e0: false, skip: 0, mods: Modifiers {
            left_shift: false,
            right_shift: false,
            left_ctrl: false,
            right_ctrl: false,
            left_alt: false,
            right_alt: false,
            caps_lock: false,
        } }
    }

    pub fn modifiers(&self) -> Modifiers {
        self.mods
    }

    /// スキャンコードを 1 バイト受け取る。キーイベントが完成したら返す
    pub fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        match byte {
            0xE0 => {
                self.e0 = true;
                return None;
            }
            0xE1 => {
                self.skip = 5;
                return None;
            }
            // キーボードからの応答（ACK, エラーなど）はキーではない。
            // 0xAA（自己テスト成功）は左 Shift の離しと同じ値なので、離しとして扱う（害はない）
            0x00 | 0xEE | 0xFA | 0xFC | 0xFD | 0xFE | 0xFF if !self.e0 => return None,
            _ => {}
        }
        let e0 = core::mem::take(&mut self.e0);
        let pressed = byte & 0x80 == 0;
        let code = (byte & 0x7F) as u16;
        // E0 2A / E0 36 は NumLock 状態などで付く偽の Shift なので無視する
        if e0 && (code == 0x2A || code == 0x36) {
            return None;
        }
        let key = KeyCode(if e0 { 0xE000 | code } else { code });

        let m = &mut self.mods;
        match key {
            KeyCode::LEFT_SHIFT => m.left_shift = pressed,
            KeyCode::RIGHT_SHIFT => m.right_shift = pressed,
            KeyCode::LEFT_CTRL => m.left_ctrl = pressed,
            KeyCode::RIGHT_CTRL => m.right_ctrl = pressed,
            KeyCode::LEFT_ALT => m.left_alt = pressed,
            KeyCode::RIGHT_ALT => m.right_alt = pressed,
            KeyCode::CAPS_LOCK if pressed => m.caps_lock = !m.caps_lock,
            _ => {}
        }
        let ch = if pressed { self.layout.char_for(key, &self.mods) } else { None };
        Some(KeyEvent { key, pressed, mods: self.mods, ch })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_bytes(bytes: &[u8]) -> String {
        let mut d = Decoder::new(&US);
        bytes.iter().filter_map(|&b| d.feed(b)).filter_map(|e| e.ch).collect()
    }

    #[test]
    fn letters_and_digits() {
        // h e l l o 1 （押して離す）
        assert_eq!(type_bytes(&[0x23, 0xA3, 0x12, 0x92, 0x26, 0xA6, 0x26, 0xA6, 0x18, 0x98, 0x02, 0x82]), "hello1");
    }

    #[test]
    fn shift_changes_symbols_and_case() {
        // Shift 押す, a, 1, Shift 離す, a
        assert_eq!(type_bytes(&[0x2A, 0x1E, 0x9E, 0x02, 0x82, 0xAA, 0x1E, 0x9E]), "A!a");
    }

    #[test]
    fn caps_lock_toggles_letters_only() {
        // CapsLock, a, 1, Shift+a, CapsLock, a
        assert_eq!(
            type_bytes(&[0x3A, 0xBA, 0x1E, 0x9E, 0x02, 0x82, 0x2A, 0x1E, 0x9E, 0xAA, 0x3A, 0xBA, 0x1E, 0x9E]),
            "A1aa"
        );
    }

    #[test]
    fn ctrl_letter_gives_control_char() {
        assert_eq!(type_bytes(&[0x1D, 0x2E, 0xAE, 0x9D]), "\u{3}");
    }

    #[test]
    fn enter_backspace_tab() {
        assert_eq!(type_bytes(&[0x1C, 0x9C, 0x0E, 0x8E, 0x0F, 0x8F]), "\n\x08\t");
    }

    #[test]
    fn extended_keys() {
        let mut d = Decoder::new(&US);
        assert_eq!(d.feed(0xE0), None);
        let e = d.feed(0x48).unwrap();
        assert_eq!((e.key, e.pressed, e.ch), (KeyCode::UP, true, None));
        d.feed(0xE0);
        let e = d.feed(0xC8).unwrap();
        assert_eq!((e.key, e.pressed), (KeyCode::UP, false));
        // テンキーの Enter と /
        assert_eq!(type_bytes(&[0xE0, 0x1C, 0xE0, 0x35]), "\n/");
    }

    #[test]
    fn right_ctrl_and_fake_shift() {
        let mut d = Decoder::new(&US);
        d.feed(0xE0);
        d.feed(0x1D);
        assert!(d.modifiers().ctrl());
        // E0 2A（偽の Shift）は無視される
        d.feed(0xE0);
        assert_eq!(d.feed(0x2A), None);
        assert!(!d.modifiers().shift());
    }

    #[test]
    fn pause_sequence_is_skipped() {
        assert_eq!(type_bytes(&[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5, 0x1E]), "a");
    }

    #[test]
    fn function_keys() {
        assert_eq!(KeyCode(0x3B).function_number(), Some(1));
        assert_eq!(KeyCode::F12.function_number(), Some(12));
        assert_eq!(KeyCode::ENTER.function_number(), None);
    }
}
