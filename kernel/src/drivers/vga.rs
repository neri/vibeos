//! VGA モード 13h の画面
//!
//! 描画はすべて裏画面（`Screen::back`）に行い、`present` で変更した行だけ VRAM に転送する。
//! 画面モードはローダが BIOS で設定済み。

use vibe_gfx::{Surface, palette};

use crate::arch::port::{inb, outb};
use crate::bootinfo::BootInfo;
use crate::sync::IrqMutex;

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 200;

const DAC_WRITE_INDEX: u16 = 0x3C8;
const DAC_DATA: u16 = 0x3C9;
const INPUT_STATUS_1: u16 = 0x3DA;
const VRETRACE: u8 = 0x08;

#[repr(C, align(16))]
pub struct Screen {
    back: [u8; WIDTH * HEIGHT],
    vram: usize,
    /// VRAM に未転送の行の範囲 [dirty_lo, dirty_hi)
    dirty_lo: usize,
    dirty_hi: usize,
}

/// 画面。CONSOLE と両方取るときは CONSOLE を先に取ること
pub static SCREEN: IrqMutex<Screen> =
    IrqMutex::new(Screen { back: [0; WIDTH * HEIGHT], vram: 0, dirty_lo: 0, dirty_hi: 0 });

pub fn init(info: &BootInfo) {
    let mut s = SCREEN.lock();
    s.vram = info.vram_addr as usize;
    set_palette(&palette::standard());
    s.surface().clear(palette::BLACK);
    s.mark_dirty(0, HEIGHT);
    s.present();
}

impl Screen {
    pub fn surface(&mut self) -> Surface<'_> {
        Surface::new(&mut self.back, WIDTH, HEIGHT)
    }

    /// 行 [y0, y1) を書き換えたことを記録する
    pub fn mark_dirty(&mut self, y0: usize, y1: usize) {
        let y1 = y1.min(HEIGHT);
        if y0 >= y1 {
            return;
        }
        if self.dirty_lo >= self.dirty_hi {
            self.dirty_lo = y0;
            self.dirty_hi = y1;
        } else {
            self.dirty_lo = self.dirty_lo.min(y0);
            self.dirty_hi = self.dirty_hi.max(y1);
        }
    }

    /// 書き換えた行を VRAM へ転送する
    pub fn present(&mut self) {
        if self.vram == 0 || self.dirty_lo >= self.dirty_hi {
            return;
        }
        let start = self.dirty_lo * WIDTH / 4;
        let end = self.dirty_hi * WIDTH / 4;
        let src = self.back.as_ptr() as *const u32;
        let dst = self.vram as *mut u32;
        for i in start..end {
            // SAFETY: back は 16 バイト境界で WIDTH*HEIGHT バイト、VRAM は 0xA0000 から 64000 バイト。
            // i は WIDTH*HEIGHT/4 未満
            unsafe { dst.add(i).write_volatile(src.add(i).read()) };
        }
        self.dirty_lo = 0;
        self.dirty_hi = 0;
    }
}

/// パレット全体を設定する（各 6bit）
pub fn set_palette(p: &[[u8; 3]; 256]) {
    // SAFETY: DAC への書き込みは色を変えるだけ
    unsafe {
        outb(DAC_WRITE_INDEX, 0);
        for c in p {
            outb(DAC_DATA, c[0]);
            outb(DAC_DATA, c[1]);
            outb(DAC_DATA, c[2]);
        }
    }
}

/// 垂直帰線期間の始まりを待つ（VGA が応答しない場合に備えて上限付き）
#[allow(dead_code)]
pub fn wait_vsync() {
    // SAFETY: ステータスレジスタを読むだけ
    unsafe {
        for _ in 0..1_000_000 {
            if inb(INPUT_STATUS_1) & VRETRACE == 0 {
                break;
            }
        }
        for _ in 0..1_000_000 {
            if inb(INPUT_STATUS_1) & VRETRACE != 0 {
                break;
            }
        }
    }
}
