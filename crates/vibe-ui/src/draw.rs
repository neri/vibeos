//! 描画の共通部分（色・立体の枠・文字）

use vibe_font::FONT_8X8;
use vibe_gfx::{Surface, palette};

use crate::{Align, Rect};

/// 部品の地の色・明るい縁・暗い縁・文字
pub const FACE: u8 = palette::LIGHT_GRAY;
pub const LIGHT: u8 = palette::WHITE;
pub const SHADOW: u8 = palette::DARK_GRAY;
pub const TEXT: u8 = palette::BLACK;
pub const TEXT_DISABLED: u8 = palette::DARK_GRAY;
/// テキスト欄・一覧の地
pub const FIELD: u8 = palette::WHITE;
/// 一覧で選んだ行
pub const SELECT: u8 = palette::BLUE;
pub const SELECT_TEXT: u8 = palette::WHITE;

pub const CHAR_W: i32 = FONT_8X8.width as i32;
pub const CHAR_H: i32 = FONT_8X8.height as i32;
/// 複数行のラベルの行の間隔
pub const LINE_H: i32 = CHAR_H + 2;

/// 立体の枠（`raised` なら出っ張り、そうでなければへこみ）
pub fn bevel(s: &mut Surface, r: Rect, raised: bool) {
    let (tl, br) = if raised { (LIGHT, SHADOW) } else { (SHADOW, LIGHT) };
    s.hline(r.x, r.y, r.w, tl);
    s.vline(r.x, r.y, r.h, tl);
    s.hline(r.x, r.y + r.h - 1, r.w, br);
    s.vline(r.x + r.w - 1, r.y, r.h, br);
}

/// 文字を描いて、終わりの x を返す
pub fn text(s: &mut Surface, x: i32, y: i32, text: &str, color: u8) -> i32 {
    s.draw_text(&FONT_8X8, x, y, text, color, None)
}

/// `r` の幅の中で寄せて 1 行描く（入りきらない分は切る）
pub fn text_aligned(s: &mut Surface, r: Rect, y: i32, text: &str, align: Align, color: u8) {
    let room = (r.w / CHAR_W).max(0) as usize;
    let len = text.chars().count().min(room);
    let w = len as i32 * CHAR_W;
    let x = match align {
        Align::Left => r.x,
        Align::Center => r.x + (r.w - w) / 2,
        Align::Right => r.x + r.w - w,
    };
    // 右寄せで入りきらなければ末尾を見せる（電卓の表示など）
    let skip = if align == Align::Right { text.chars().count() - len } else { 0 };
    let shown: alloc::string::String = text.chars().skip(skip).take(len).collect();
    s.draw_text(&FONT_8X8, x, y, &shown, color, None);
}
