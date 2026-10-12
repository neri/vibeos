//! 全画面の画面（gfx_open / gfx_blit）を使うための入れ物

use alloc::vec;
use alloc::vec::Vec;

use vibe_abi::{GFX_CURSOR, SCREEN_HEIGHT, SCREEN_WIDTH};
use vibe_gfx::Surface;

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
        let id = sys::win_open(w as u32, h as u32, title);
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

/// 画面がデスクトップ（ウィンドウを重ねて出す）か。320x200 の全画面なら false
pub fn is_desktop() -> bool {
    sys::sys_info().flags & vibe_abi::SYS_DESKTOP != 0
}
