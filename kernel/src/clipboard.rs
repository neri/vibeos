//! クリップボード（カーネルに 1 つ。端末・edit・テキスト欄の間で文字をやりとりする）

use alloc::string::String;

use crate::sync::IrqMutex;

/// 入れておける大きさ
pub const MAX: usize = 4096;

static CLIP: IrqMutex<String> = IrqMutex::new(String::new());

/// 入れる（長すぎる分は切る。文字の途中では切らない）
pub fn set(text: &str) {
    let mut end = text.len().min(MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut c = CLIP.lock();
    c.clear();
    c.push_str(&text[..end]);
}

pub fn get() -> String {
    CLIP.lock().clone()
}
