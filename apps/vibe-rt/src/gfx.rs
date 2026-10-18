//! ウィンドウ（win_open / win_blit / win_event）と、アプリの文字（8x16）・アイコン（16x16）。
//! 色は 16 色の番号で描く（画面は白黒で、黒と白以外は明るさの模様になる。docs/plans/mono.md）

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

use vibe_abi::{SCREEN_HEIGHT, SCREEN_WIDTH};
use vibe_font::Font;
use vibe_gfx::Surface;

use crate::sys;

/// アプリの既定のウィンドウの大きさ（320x200）
pub const WIDTH: usize = SCREEN_WIDTH;
pub const HEIGHT: usize = SCREEN_HEIGHT;

/// 好きな大きさのウィンドウ（win_open）。捨てると閉じる
pub struct Window {
    id: i32,
    w: usize,
    h: usize,
    buf: Vec<u8>,
}

/// ウィンドウのイベント
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(vibe_abi::KeyEvent),
    /// 中身の座標とボタン（bit0 = 左, bit1 = 右, bit2 = 中）
    Mouse { x: i32, y: i32, buttons: u32 },
    /// [x] が押された
    Close,
}

impl Window {
    pub fn open(w: usize, h: usize, title: &str) -> Option<Window> {
        let id = sys::win_open(w as u32, h as u32, title, 0);
        (id >= 0).then(|| Window { id, w, h, buf: vec![0; w * h] })
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    pub fn surface(&mut self) -> Surface<'_> {
        Surface::new(&mut self.buf, self.w, self.h)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.buf
    }

    pub fn present(&self) {
        sys::win_blit(self.id, &self.buf);
    }

    /// タイトルを変える
    pub fn set_title(&self, title: &str) {
        sys::win_title(self.id, title);
    }

    /// イベントを待つ（timeout_ms = 0 なら待たない、None ならいつまでも）
    pub fn event(&self, timeout_ms: Option<u32>) -> Option<Event> {
        let ev = sys::win_event(self.id, timeout_ms.unwrap_or(u32::MAX))?;
        match ev.kind {
            vibe_abi::WIN_EVENT_KEY => Some(Event::Key(vibe_abi::KeyEvent(ev.key))),
            vibe_abi::WIN_EVENT_MOUSE => Some(Event::Mouse { x: ev.x, y: ev.y, buttons: ev.buttons }),
            vibe_abi::WIN_EVENT_CLOSE => Some(Event::Close),
            _ => None,
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        sys::win_close(self.id);
    }
}

static FONT16: AtomicPtr<Font> = AtomicPtr::new(core::ptr::null_mut());

/// 8x16 のフォント（VGA の BIOS のもの。カーネルから最初に使うときに写し、ずっと持つ）。無ければ 8x8 を
/// 縦に 2 倍したもの。アプリの文字
pub fn font16() -> &'static Font {
    let p = FONT16.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: 入れるのは Box::leak したものだけで、消さない
        return unsafe { &*p };
    }
    let mut buf = Box::new([0u8; 256 * 16]);
    let font = if sys::font_data(vibe_abi::FONT_VGA16, &mut buf[..]) == buf.len() as i32 {
        Font::vga16(Box::leak(buf))
    } else {
        vibe_font::FONT_8X16_TALL.clone()
    };
    let f: &'static Font = Box::leak(Box::new(font));
    FONT16.store(f as *const _ as *mut _, Ordering::Release);
    f
}

/// 8x16 の文字列の幅
pub fn text16_width(s: &str) -> i32 {
    (s.chars().count() * font16().width) as i32
}

/// 8x16 の文字列を `x`〜`x + w` の真ん中に描く（`bold` なら重ね打ち）
pub fn draw_text16_centered(s: &mut Surface, x: i32, w: i32, y: i32, text: &str, color: u8, bold: bool) {
    let x = x + (w - text16_width(text)) / 2;
    if bold {
        s.draw_text_bold(font16(), x, y, text, color);
    } else {
        s.draw_text(font16(), x, y, text, color, None);
    }
}

/// 16x16 のアイコン（色番号。今の絵は黒と白だけ。透明は `vibe_gfx::icons::TRANSPARENT`）。無ければ None
pub fn icon16(name: &str) -> Option<Vec<u8>> {
    let mut buf = [0u8; vibe_abi::ICON_SIZE * vibe_abi::ICON_SIZE];
    (sys::icon16_data(name, &mut buf) == 0).then(|| buf.to_vec())
}

/// アイコンを (x, y) に置く（透明の画素は描かない）
pub fn draw_icon16(s: &mut Surface, x: i32, y: i32, icon: &[u8]) {
    s.blit(x, y, icon, vibe_abi::ICON_SIZE, Some(vibe_gfx::icons::TRANSPARENT));
}
