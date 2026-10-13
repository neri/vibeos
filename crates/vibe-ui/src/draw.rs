//! 描画の共通部分（色・文字・角の丸い枠）
//!
//! 部品は 32 ビットの `Canvas` に、`vibe_gfx::theme` の色で描く（明るい地・強調色の青・角の丸い枠）。
//! 文字はなめらかなフォント（`set_fonts` でアプリが 1 回渡す。カーネルが読み込んだものを vibe_rt で取る）。
//! 渡されていなければ 8x8 のビットマップのフォント（ホストのテストもこちら）。

use core::sync::atomic::{AtomicPtr, Ordering};

use vibe_font::{FONT_8X8, VFont, vfn::SUBPIXELS};
use vibe_gfx::canvas::{Canvas, Color, Corner, rgb};
use vibe_gfx::theme;

use crate::{Align, Rect};

/// ウィンドウの地
pub const FACE: Color = theme::FACE;
/// テキスト欄・一覧・表示欄・ボタンの地
pub const FIELD: Color = rgb(255, 255, 255);
/// 枠の線
pub const LINE: Color = rgb(198, 201, 208);
pub const TEXT: Color = theme::TEXT;
pub const TEXT_DISABLED: Color = theme::TEXT_DIM;
/// マウスが乗ったボタン・押したボタンの地
pub const BUTTON_HOVER: Color = rgb(240, 242, 246);
pub const BUTTON_PRESSED: Color = rgb(222, 225, 232);
/// 選んだもの（フォーカスがあるとき・無いとき）
pub const SELECT: Color = theme::ACCENT;
pub const SELECT_TEXT: Color = rgb(255, 255, 255);
pub const SELECT_INACTIVE: Color = theme::ACCENT_LIGHT;
/// スクロールバーのつまみ
pub const THUMB: Color = rgb(192, 196, 204);
/// 角の半径
pub const RADIUS: usize = theme::RADIUS_SMALL;

static FONT: AtomicPtr<VFont<'static>> = AtomicPtr::new(core::ptr::null_mut());
static BOLD: AtomicPtr<VFont<'static>> = AtomicPtr::new(core::ptr::null_mut());

/// 部品の文字に使うフォント（ふつうと太字）を決める。アプリが最初に 1 回呼ぶ
pub fn set_fonts(font: Option<&'static VFont<'static>>, bold: Option<&'static VFont<'static>>) {
    let p = |f: Option<&'static VFont<'static>>| f.map_or(core::ptr::null_mut(), |f| f as *const _ as *mut _);
    FONT.store(p(font), Ordering::Release);
    BOLD.store(p(bold.or(font)), Ordering::Release);
}

fn load(slot: &AtomicPtr<VFont<'static>>) -> Option<&'static VFont<'static>> {
    let p = slot.load(Ordering::Acquire);
    // SAFETY: 入れるのは &'static VFont だけ
    (!p.is_null()).then(|| unsafe { &*p })
}

/// ふつうの文字
pub fn text() -> Text {
    Text(load(&FONT))
}

/// 太字
pub fn bold() -> Text {
    Text(load(&BOLD))
}

/// 文字の測り方と描き方（なめらかなフォントか、無ければ 8x8）
#[derive(Clone, Copy)]
pub struct Text(Option<&'static VFont<'static>>);

impl Text {
    /// 1 行の高さ
    pub fn height(&self) -> i32 {
        self.0.map_or(FONT_8X8.height as i32, |f| f.line_height as i32)
    }

    /// 先頭から `n` 文字の幅
    pub fn prefix_width(&self, s: &str, n: usize) -> i32 {
        match self.0 {
            Some(f) => {
                let pen: i32 = s.chars().take(n).filter_map(|c| f.glyph_or_replacement(c)).map(|g| g.advance as i32).sum();
                (pen + SUBPIXELS / 2) / SUBPIXELS
            }
            None => (s.chars().take(n).count() * FONT_8X8.width) as i32,
        }
    }

    pub fn width(&self, s: &str) -> i32 {
        self.prefix_width(s, usize::MAX)
    }

    /// 幅 `max` に入る先頭の文字数
    pub fn fit(&self, s: &str, max: i32) -> usize {
        match self.0 {
            Some(f) => f.fit(s, max),
            None => (max.max(0) as usize / FONT_8X8.width).min(s.chars().count()),
        }
    }

    /// 左端から `x` の所に一番近い文字の境目（何文字目の前か）
    pub fn index_at(&self, s: &str, x: i32) -> usize {
        let n = s.chars().count();
        let mut best = (i32::MAX, 0);
        for i in 0..=n {
            let d = (self.prefix_width(s, i) - x).abs();
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }

    /// 1 行描く（`top` は行の上端）。描いた幅を返す
    pub fn draw(&self, c: &mut Canvas, x: i32, top: i32, s: &str, color: Color) -> i32 {
        match self.0 {
            Some(f) => c.draw_vtext(f, x, top, s, color),
            None => c.draw_text(&FONT_8X8, x, top, s, color, None),
        }
    }

    /// `r` の中に 1 行、縦の真ん中・横は寄せて描く（入りきらない分は切る。右寄せなら末尾を見せる）
    pub fn draw_in(&self, c: &mut Canvas, r: Rect, s: &str, align: Align, color: Color) {
        let n = s.chars().count();
        let fit = self.fit(s, r.w);
        // 右寄せで入りきらなければ末尾を見せる（電卓の表示など）
        let skip = if align == Align::Right { n - fit } else { 0 };
        let shown: alloc::string::String = s.chars().skip(skip).take(fit).collect();
        let w = self.width(&shown);
        let x = match align {
            Align::Left => r.x,
            Align::Center => r.x + (r.w - w) / 2,
            Align::Right => r.x + r.w - w,
        };
        self.draw(c, x, r.y + (r.h - self.height()) / 2, &shown, color);
    }
}

/// 角の丸い枠の表（描くたびに作る。小さいので軽い）
pub struct Corners {
    pub outer: Corner,
    pub inner: Corner,
}

impl Corners {
    pub fn new() -> Corners {
        Corners { outer: Corner::new(RADIUS), inner: Corner::new(RADIUS - 1) }
    }
}

impl Default for Corners {
    fn default() -> Self {
        Self::new()
    }
}

/// 角の丸い、1 ピクセルの枠 `border` の付いた矩形を `fill` で塗る
pub fn frame(c: &mut Canvas, r: Rect, k: &Corners, fill: Color, border: Color) {
    c.fill_round_rect(r.x, r.y, r.w, r.h, &k.outer, border, 255);
    let i = r.inset(1);
    c.fill_round_rect(i.x, i.y, i.w, i.h, &k.inner, fill, 255);
}

/// フォーカスの輪（強調色の 2 ピクセルの枠。`r` の外側 1 ピクセルと内側 1 ピクセル）
pub fn focus_ring(c: &mut Canvas, r: Rect, k: &Corners) {
    let o = Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2);
    c.round_rect(o.x, o.y, o.w, o.h, &k.outer, SELECT);
    c.round_rect(r.x, r.y, r.w, r.h, &k.outer, SELECT);
}
