//! 描画の共通部分（色・文字・立体の枠）
//!
//! 部品は 8 ビットの `Surface` に VGA の 16 色（`palette::VGA16` の番号）で描く。Windows 3.x 風: 明るい灰色の
//! 地に、白と濃い灰色の縁の立体のボタン。テキスト欄・一覧・表示欄は白い地にへこんだ枠。フォーカスは黒の点線の
//! 枠、選んだものは紺の地に白い文字（docs/plans/vga16.md、D-072）。
//! 文字は固定幅のビットマップのフォント（`set_font` でアプリが 1 回渡す。16 色の画面では 8x16）。
//! 渡されていなければ 8x8（ホストのテストもこちら）。太字は 1 ピクセル右にずらして重ねる。

use core::sync::atomic::{AtomicPtr, Ordering};

use vibe_font::{FONT_8X8, Font};
use vibe_gfx::Surface;
use vibe_gfx::palette::{BLACK, BLUE, DARK_GRAY, LIGHT_GRAY, WHITE};

use crate::{Align, Rect};

/// ウィンドウ・ボタンの地
pub const FACE: u8 = LIGHT_GRAY;
/// テキスト欄・一覧・表示欄の地
pub const FIELD: u8 = WHITE;
pub const TEXT: u8 = BLACK;
pub const TEXT_DISABLED: u8 = DARK_GRAY;
/// 立体の縁: 光の当たる側・影の側（内側）・一番外の影
pub const LIGHT: u8 = WHITE;
pub const SHADOW: u8 = DARK_GRAY;
pub const DARK: u8 = BLACK;
/// フォーカスの点線の枠・チェックボックスの印
pub const FOCUS: u8 = BLACK;
/// 選んだもの（一覧の行・テキスト欄の全体の選択）: 紺の地に白い文字。一覧にフォーカスが無ければ明るい灰色の地に
/// ふつうの文字
pub const SELECT: u8 = BLUE;
pub const SELECT_TEXT: u8 = WHITE;
pub const SELECT_INACTIVE: u8 = LIGHT_GRAY;

static FONT: AtomicPtr<Font> = AtomicPtr::new(core::ptr::null_mut());

/// 部品の文字に使うフォントを決める。アプリが最初に 1 回呼ぶ
pub fn set_font(font: &'static Font) {
    FONT.store(font as *const _ as *mut _, Ordering::Release);
}

fn font() -> &'static Font {
    let p = FONT.load(Ordering::Acquire);
    // SAFETY: 入れるのは &'static Font だけ
    if p.is_null() { &FONT_8X8 } else { unsafe { &*p } }
}

/// ふつうの文字
pub fn text() -> Text {
    Text { font: font(), bold: false }
}

/// 太字（重ね打ち。幅はふつうの文字と同じ）
pub fn bold() -> Text {
    Text { font: font(), bold: true }
}

/// 文字の測り方と描き方（固定幅のビットマップのフォント）
#[derive(Clone, Copy)]
pub struct Text {
    font: &'static Font,
    bold: bool,
}

impl Text {
    /// 1 行の高さ
    pub fn height(&self) -> i32 {
        self.font.height as i32
    }

    /// 先頭から `n` 文字の幅
    pub fn prefix_width(&self, s: &str, n: usize) -> i32 {
        (s.chars().take(n).count() * self.font.width) as i32
    }

    pub fn width(&self, s: &str) -> i32 {
        self.prefix_width(s, usize::MAX)
    }

    /// 幅 `max` に入る先頭の文字数
    pub fn fit(&self, s: &str, max: i32) -> usize {
        (max.max(0) as usize / self.font.width).min(s.chars().count())
    }

    /// 左端から `x` の所に一番近い文字の境目（何文字目の前か）
    pub fn index_at(&self, s: &str, x: i32) -> usize {
        let w = self.font.width as i32;
        ((x + w / 2).max(0) / w).min(s.chars().count() as i32) as usize
    }

    /// 1 行描く（`top` は行の上端）。描いた幅を返す
    pub fn draw(&self, s: &mut Surface, x: i32, top: i32, text: &str, color: u8) -> i32 {
        if self.bold {
            s.draw_text_bold(self.font, x, top, text, color)
        } else {
            s.draw_text(self.font, x, top, text, color, None)
        }
    }

    /// `r` の中に 1 行、縦の真ん中・横は寄せて描く（入りきらない分は切る。右寄せなら末尾を見せる）
    pub fn draw_in(&self, s: &mut Surface, r: Rect, text: &str, align: Align, color: u8) {
        let n = text.chars().count();
        let fit = self.fit(text, r.w);
        // 右寄せで入りきらなければ末尾を見せる（電卓の表示など）
        let skip = if align == Align::Right { n - fit } else { 0 };
        let shown: alloc::string::String = text.chars().skip(skip).take(fit).collect();
        let w = self.width(&shown);
        let x = match align {
            Align::Left => r.x,
            Align::Center => r.x + (r.w - w) / 2,
            Align::Right => r.x + r.w - w,
        };
        self.draw(s, x, r.y + (r.h - self.height()) / 2, &shown, color);
    }
}

/// 出っ張った立体の枠（2 ピクセル。外は白と黒、内は地と濃い灰色）。中は塗らない
pub fn raised(s: &mut Surface, r: Rect) {
    s.bevel(r.x, r.y, r.w, r.h, LIGHT, DARK);
    s.bevel(r.x + 1, r.y + 1, r.w - 2, r.h - 2, FACE, SHADOW);
}

/// 押したボタンの枠（2 ピクセル。外は黒と白、内は濃い灰色と地）
pub fn pressed(s: &mut Surface, r: Rect) {
    s.bevel(r.x, r.y, r.w, r.h, DARK, LIGHT);
    s.bevel(r.x + 1, r.y + 1, r.w - 2, r.h - 2, SHADOW, FACE);
}

/// へこんだ欄の枠（2 ピクセル。外は濃い灰色と白、内は黒と地）。中は塗らない
pub fn sunken(s: &mut Surface, r: Rect) {
    s.bevel(r.x, r.y, r.w, r.h, SHADOW, LIGHT);
    s.bevel(r.x + 1, r.y + 1, r.w - 2, r.h - 2, DARK, FACE);
}

/// フォーカスの点線の枠（1 つおきの点）
pub fn focus_rect(s: &mut Surface, r: Rect) {
    if r.w <= 0 || r.h <= 0 {
        return;
    }
    for x in r.x..r.x + r.w {
        if (x - r.x) % 2 == 0 {
            s.put_pixel(x, r.y, FOCUS);
            s.put_pixel(x, r.y + r.h - 1, FOCUS);
        }
    }
    for y in r.y..r.y + r.h {
        if (y - r.y) % 2 == 0 {
            s.put_pixel(r.x, y, FOCUS);
            s.put_pixel(r.x + r.w - 1, y, FOCUS);
        }
    }
}
