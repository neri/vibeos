//! 描画の共通部分（色・文字・枠）
//!
//! 部品は 8 ビットの `Surface` に色番号で描く。画面は白黒（docs/plans/mono.md、D-077）なので、使う色は黒・白と
//! 濃い灰（画面では 50% の市松。使えない字）だけ。見た目は「今風の 1 ビット」: 白い地に 2 ピクセルの黒い枠、
//! ボタンは角の丸い枠と右下にずらした黒い影（押している間は影の所へずれて白黒が入れ替わる）。選んだものは
//! 黒い地に白い字（フォーカスが無ければ黒い枠だけ）、フォーカスは黒の点線の枠。部品の大きさは前のまま。
//! 文字は固定幅のビットマップのフォント（`set_font` でアプリが 1 回渡す。白黒の画面では 8x16）。
//! 渡されていなければ 8x8（ホストのテストもこちら）。太字は 1 ピクセル右にずらして重ねる。

use core::sync::atomic::{AtomicPtr, Ordering};

use vibe_font::{FONT_8X8, Font};
use vibe_gfx::Surface;
use vibe_gfx::palette::{BLACK, DARK_GRAY, WHITE};

use crate::{Align, Rect};

/// ウィンドウ・ボタンの地
pub const FACE: u8 = WHITE;
/// テキスト欄・一覧・表示欄の地
pub const FIELD: u8 = WHITE;
pub const TEXT: u8 = BLACK;
/// 使えない字（画面では 50% の市松）
pub const TEXT_DISABLED: u8 = DARK_GRAY;
/// 枠と影
pub const LINE: u8 = BLACK;
/// 枠の太さ、ボタンの角の丸み、ボタンの影（右下にずらす量）
pub const LINE_W: i32 = 2;
pub const BUTTON_RADIUS: i32 = 3;
pub const BUTTON_SHADOW: i32 = 2;
/// フォーカスの点線の枠・チェックボックスの印
pub const FOCUS: u8 = BLACK;
/// 選んだもの（一覧の行・テキスト欄の全体の選択）: 黒い地に白い字。一覧にフォーカスが無ければ黒い枠だけ
pub const SELECT: u8 = BLACK;
pub const SELECT_TEXT: u8 = WHITE;

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

/// テキスト欄・一覧・表示欄の枠（2 ピクセルの黒）。中は塗らない
pub fn frame(s: &mut Surface, r: Rect) {
    s.round_rect(r.x, r.y, r.w, r.h, 0, LINE_W, LINE);
}

/// ボタンの見た目の状態
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonLook {
    Normal,
    /// 既定のボタン（枠が 1 ピクセル太い）
    Primary,
    /// 押している間
    Pressed,
}

/// ボタンの枠と影を `r` の中に描き、字を置く矩形（ボタンの本体）を返す。本体は `r` から影の分だけ小さく、
/// 押している間は影の所へずれて黒く塗る
pub fn button(s: &mut Surface, r: Rect, look: ButtonLook) -> Rect {
    s.fill_rect(r.x, r.y, r.w, r.h, FACE);
    let (w, h) = (r.w - BUTTON_SHADOW, r.h - BUTTON_SHADOW);
    if look == ButtonLook::Pressed {
        let body = Rect::new(r.x + BUTTON_SHADOW, r.y + BUTTON_SHADOW, w, h);
        s.fill_round_rect(body.x, body.y, w, h, BUTTON_RADIUS, LINE);
        return body;
    }
    s.fill_round_rect(r.x + BUTTON_SHADOW, r.y + BUTTON_SHADOW, w, h, BUTTON_RADIUS, LINE);
    s.fill_round_rect(r.x, r.y, w, h, BUTTON_RADIUS, FACE);
    let t = if look == ButtonLook::Primary { LINE_W + 1 } else { LINE_W };
    s.round_rect(r.x, r.y, w, h, BUTTON_RADIUS, t, LINE);
    Rect::new(r.x, r.y, w, h)
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
