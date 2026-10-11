//! US 配列

use crate::{KeyCode, Layout, Modifiers};

pub struct UsLayout;

pub static US: UsLayout = UsLayout;

/// セット 1 のメイクコード 0x00〜0x58 → (通常, Shift)。'\0' は文字なし
const TABLE: [(char, char); 0x59] = [
    ('\0', '\0'),     // 00
    ('\x1b', '\x1b'), // 01 Esc
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    ('0', ')'),
    ('-', '_'),
    ('=', '+'),
    ('\x08', '\x08'), // 0E Backspace
    ('\t', '\t'),     // 0F Tab
    ('q', 'Q'),
    ('w', 'W'),
    ('e', 'E'),
    ('r', 'R'),
    ('t', 'T'),
    ('y', 'Y'),
    ('u', 'U'),
    ('i', 'I'),
    ('o', 'O'),
    ('p', 'P'),
    ('[', '{'),
    (']', '}'),
    ('\n', '\n'), // 1C Enter
    ('\0', '\0'), // 1D LCtrl
    ('a', 'A'),
    ('s', 'S'),
    ('d', 'D'),
    ('f', 'F'),
    ('g', 'G'),
    ('h', 'H'),
    ('j', 'J'),
    ('k', 'K'),
    ('l', 'L'),
    (';', ':'),
    ('\'', '"'),
    ('`', '~'),
    ('\0', '\0'), // 2A LShift
    ('\\', '|'),
    ('z', 'Z'),
    ('x', 'X'),
    ('c', 'C'),
    ('v', 'V'),
    ('b', 'B'),
    ('n', 'N'),
    ('m', 'M'),
    (',', '<'),
    ('.', '>'),
    ('/', '?'),
    ('\0', '\0'), // 36 RShift
    ('*', '*'),   // 37 テンキー *
    ('\0', '\0'), // 38 LAlt
    (' ', ' '),   // 39 Space
    ('\0', '\0'), // 3A CapsLock
    ('\0', '\0'), // 3B F1
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'),
    ('\0', '\0'), // 44 F10
    ('\0', '\0'), // 45 NumLock
    ('\0', '\0'), // 46 ScrollLock
    ('7', '7'),   // 47 テンキー（NumLock は常に有効として扱う）
    ('8', '8'),
    ('9', '9'),
    ('-', '-'),
    ('4', '4'),
    ('5', '5'),
    ('6', '6'),
    ('+', '+'),
    ('1', '1'),
    ('2', '2'),
    ('3', '3'),
    ('0', '0'),
    ('.', '.'),   // 53
    ('\0', '\0'), // 54
    ('\0', '\0'), // 55
    ('\0', '\0'), // 56
    ('\0', '\0'), // 57 F11
    ('\0', '\0'), // 58 F12
];

impl Layout for UsLayout {
    fn char_for(&self, key: KeyCode, mods: &Modifiers) -> Option<char> {
        match key {
            KeyCode::KEYPAD_ENTER => return Some('\n'),
            KeyCode::KEYPAD_SLASH => return Some('/'),
            _ => {}
        }
        let (normal, shifted) = *TABLE.get(key.0 as usize)?;
        if normal == '\0' {
            return None;
        }
        if normal.is_ascii_lowercase() {
            if mods.ctrl() {
                return Some(((normal as u8) & 0x1F) as char);
            }
            return Some(if mods.shift() != mods.caps_lock { shifted } else { normal });
        }
        Some(if mods.shift() { shifted } else { normal })
    }
}
