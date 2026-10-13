//! 全画面の画面（gfx_open / gfx_blit）を使うための入れ物

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

use vibe_abi::{GFX_CURSOR, SCREEN_HEIGHT, SCREEN_WIDTH};
use vibe_font::VFont;
use vibe_gfx::Surface;
use vibe_gfx::canvas::Canvas;

use crate::sys;

pub const WIDTH: usize = SCREEN_WIDTH;
pub const HEIGHT: usize = SCREEN_HEIGHT;

/// アプリ側の裏バッファ。`present` で画面に送る。捨てるとコンソールに戻る
pub struct Screen {
    buf: Vec<u8>,
}

impl Screen {
    /// 画面を開く。`cursor` ならマウスカーソルを表示する
    pub fn open(cursor: bool) -> Option<Screen> {
        if sys::gfx_open(if cursor { GFX_CURSOR } else { 0 }) < 0 {
            return None;
        }
        Some(Screen { buf: vec![0; WIDTH * HEIGHT] })
    }

    pub fn surface(&mut self) -> Surface<'_> {
        Surface::new(&mut self.buf, WIDTH, HEIGHT)
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    pub fn present(&self) {
        let frame: &[u8; WIDTH * HEIGHT] = self.buf.as_slice().try_into().unwrap();
        sys::gfx_blit(frame);
    }

    /// パレットを変える（各 6bit）。画面を閉じると標準に戻る
    pub fn set_palette(&self, start: u8, colors: &[[u8; 3]]) {
        sys::gfx_palette(start, colors);
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        sys::gfx_close();
    }
}

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

    pub fn present(&self) {
        sys::win_blit(self.id, &self.buf);
    }

    /// ウィンドウ独自のパレット（各 6bit）。デスクトップでは標準のパレットの近い色で出る
    pub fn set_palette(&self, start: u8, colors: &[[u8; 3]]) {
        sys::win_palette(self.id, start, colors);
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

/// 中身が 32 ビット（0x00RRGGBB）のウィンドウ（win_open の WIN_RGB32）。捨てると閉じる。
/// 320x200 の全画面では標準のパレットの近い色で出る
pub struct Window32 {
    id: i32,
    w: usize,
    h: usize,
    buf: Vec<u32>,
}

impl Window32 {
    pub fn open(w: usize, h: usize, title: &str) -> Option<Window32> {
        let id = sys::win_open(w as u32, h as u32, title, vibe_abi::WIN_RGB32);
        (id >= 0).then(|| Window32 { id, w, h, buf: vec![0; w * h] })
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    pub fn canvas(&mut self) -> Canvas<'_> {
        Canvas::new(&mut self.buf, self.w, self.h)
    }

    pub fn present(&self) {
        sys::win_blit32(self.id, &self.buf);
    }

    /// イベントを待つ（timeout_ms = 0 なら待たない、None ならいつまでも）。マウスがウィンドウから出ると
    /// `Mouse { x: -1, y: -1, .. }`
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

impl Drop for Window32 {
    fn drop(&mut self) {
        sys::win_close(self.id);
    }
}

static FONTS: [AtomicPtr<VFont<'static>>; 3] =
    [AtomicPtr::new(core::ptr::null_mut()), AtomicPtr::new(core::ptr::null_mut()), AtomicPtr::new(core::ptr::null_mut())];

/// カーネルが読み込んだフォント（kind は vibe_abi::FONT_*）。最初に使うときにカーネルから写し、ずっと持つ。
/// 無ければ None（8x8 のビットマップのフォントで描く）
pub fn font(kind: u32) -> Option<&'static VFont<'static>> {
    let slot = FONTS.get(kind as usize)?;
    let p = slot.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: 入れるのは Box::leak したものだけで、消さない
        return Some(unsafe { &*p });
    }
    let size = sys::font_data(kind, &mut []);
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if sys::font_data(kind, &mut buf) != size {
        return None;
    }
    let data: &'static [u8] = Box::leak(buf.into_boxed_slice());
    let font: &'static VFont<'static> = Box::leak(Box::new(VFont::parse(data).ok()?));
    slot.store(font as *const _ as *mut _, Ordering::Release);
    Some(font)
}

/// 16x16 のアイコン（事前に掛けたアルファの 0xAARRGGBB）。無ければ None
pub fn icon(name: &str) -> Option<Vec<u32>> {
    let mut buf = [0u32; vibe_abi::ICON_SIZE * vibe_abi::ICON_SIZE];
    (sys::icon_data(name, &mut buf) == 0).then(|| buf.to_vec())
}

/// 画面がデスクトップ（ウィンドウを重ねて出す）か。320x200 の全画面なら false
pub fn is_desktop() -> bool {
    sys::sys_info().flags & vibe_abi::SYS_DESKTOP != 0
}
