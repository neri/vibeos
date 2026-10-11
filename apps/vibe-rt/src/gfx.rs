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
