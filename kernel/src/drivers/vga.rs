//! VGA モード 13h の画面と「画面（ディスプレイ）」の切り替え
//!
//! - 画面は複数持てる。0 番はコンソール（ステータスバー + 文字）で、静的な領域を使う。
//!   アプリは全画面の画面を作って前面に出す（DESIGN.md §8.4「全画面アプリの切り替え」）
//! - 描画はそれぞれの画面の裏バッファに行い、`present` で前面の画面の変更した行だけ VRAM に転送する
//! - マウスカーソルは転送するときに VRAM の上にだけ描く（裏バッファは汚さない）
//!
//! 画面モードはローダが BIOS で設定済み。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;

use vibe_gfx::{Surface, palette};

use crate::arch::port::{inb, outb};
use crate::bootinfo::BootInfo;
use crate::sync::IrqMutex;

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 200;

pub type DisplayId = u32;
pub const CONSOLE: DisplayId = 0;

const DAC_WRITE_INDEX: u16 = 0x3C8;
const DAC_DATA: u16 = 0x3C9;
const INPUT_STATUS_1: u16 = 0x3DA;
const VRETRACE: u8 = 0x08;

/// マウスカーソル（矢印）。'#' = 黒の縁, 'o' = 白
const CURSOR_SHAPE: [&str; 12] = [
    "#         ",
    "##        ",
    "#o#       ",
    "#oo#      ",
    "#ooo#     ",
    "#oooo#    ",
    "#ooooo#   ",
    "#oooooo#  ",
    "#ooo####  ",
    "#o#o#     ",
    "## #o#    ",
    "    ##    ",
];
const CURSOR_H: usize = CURSOR_SHAPE.len();

#[repr(C, align(16))]
pub struct Screen {
    console: [u8; WIDTH * HEIGHT],
    apps: BTreeMap<DisplayId, Box<[u8]>>,
    next_id: DisplayId,
    foreground: DisplayId,
    vram: usize,
    /// VRAM に未転送の行の範囲 [dirty_lo, dirty_hi)
    dirty_lo: usize,
    dirty_hi: usize,
    cursor_x: i32,
    cursor_y: i32,
    cursor_visible: bool,
}

/// 画面。CONSOLE と両方取るときは CONSOLE を先に取ること
///
/// 初期値はすべて 0 にしておく（0 でない値があると 64KB のバッファごと .data に入り、カーネルの
/// ファイルが大きくなる）。0 でない初期値は `init` で設定する。
pub static SCREEN: IrqMutex<Screen> = IrqMutex::new(Screen {
    console: [0; WIDTH * HEIGHT],
    apps: BTreeMap::new(),
    next_id: 0,
    foreground: CONSOLE,
    vram: 0,
    dirty_lo: 0,
    dirty_hi: 0,
    cursor_x: 0,
    cursor_y: 0,
    cursor_visible: false,
});

pub fn init(info: &BootInfo) {
    let mut s = SCREEN.lock();
    s.vram = info.vram_addr as usize;
    s.cursor_x = (WIDTH / 2) as i32;
    s.cursor_y = (HEIGHT / 2) as i32;
    set_palette(&palette::standard());
    s.surface().clear(palette::BLACK);
    s.mark_dirty(0, HEIGHT);
    s.present();
}

impl Screen {
    /// コンソールの画面
    pub fn surface(&mut self) -> Surface<'_> {
        Surface::new(&mut self.console, WIDTH, HEIGHT)
    }

    /// コンソールの画面の行 [y0, y1) を書き換えたことを記録する
    pub fn mark_dirty(&mut self, y0: usize, y1: usize) {
        self.mark_display_dirty(CONSOLE, y0, y1);
    }

    /// 全画面の画面を作る（まだ前面には出さない）
    pub fn create_display(&mut self) -> DisplayId {
        self.next_id += 1; // 0 はコンソールなので 1 から
        let id = self.next_id;
        // 64KB をスタックに置かないよう、ヒープに直接作る
        self.apps.insert(id, vec![palette::BLACK; WIDTH * HEIGHT].into_boxed_slice());
        id
    }

    /// 画面を捨てる。前面だったらコンソールに戻す
    pub fn destroy_display(&mut self, id: DisplayId) {
        if id == CONSOLE {
            return;
        }
        self.apps.remove(&id);
        if self.foreground == id {
            self.set_foreground(CONSOLE);
        }
    }

    pub fn display_surface(&mut self, id: DisplayId) -> Option<Surface<'_>> {
        if id == CONSOLE {
            return Some(self.surface());
        }
        self.apps.get_mut(&id).map(|b| Surface::new(b, WIDTH, HEIGHT))
    }

    pub fn mark_display_dirty(&mut self, id: DisplayId, y0: usize, y1: usize) {
        if id == self.foreground {
            self.mark_rows(y0, y1);
        }
    }

    pub fn set_foreground(&mut self, id: DisplayId) {
        if id == CONSOLE || self.apps.contains_key(&id) {
            self.foreground = id;
            self.mark_rows(0, HEIGHT);
        }
    }

    #[allow(dead_code)]
    pub fn foreground(&self) -> DisplayId {
        self.foreground
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        if self.cursor_visible != visible {
            self.cursor_visible = visible;
            self.mark_cursor_rows();
        }
    }

    pub fn set_cursor(&mut self, x: i32, y: i32) {
        if (x, y) == (self.cursor_x, self.cursor_y) {
            return;
        }
        self.mark_cursor_rows();
        self.cursor_x = x;
        self.cursor_y = y;
        self.mark_cursor_rows();
    }

    fn mark_cursor_rows(&mut self) {
        let y = self.cursor_y.max(0) as usize;
        self.mark_rows(y, y + CURSOR_H);
    }

    fn mark_rows(&mut self, y0: usize, y1: usize) {
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

    /// 前面の画面の書き換えた行を VRAM へ転送し、カーソルを重ねる
    pub fn present(&mut self) {
        if self.vram == 0 || self.dirty_lo >= self.dirty_hi {
            return;
        }
        let (lo, hi) = (self.dirty_lo, self.dirty_hi);
        let src: &[u8] = match self.apps.get(&self.foreground) {
            Some(b) => b,
            None => &self.console,
        };
        let src = src.as_ptr() as *const u32;
        let dst = self.vram as *mut u32;
        for i in lo * WIDTH / 4..hi * WIDTH / 4 {
            // SAFETY: どの画面のバッファも WIDTH*HEIGHT バイト（4 の倍数の位置から 4 バイトずつ読む）、
            // VRAM は 0xA0000 から 64000 バイト。i は WIDTH*HEIGHT/4 未満
            unsafe { dst.add(i).write_volatile(src.add(i).read_unaligned()) };
        }
        if self.cursor_visible {
            self.draw_cursor(lo, hi);
        }
        self.dirty_lo = 0;
        self.dirty_hi = 0;
    }

    fn draw_cursor(&self, lo: usize, hi: usize) {
        let vram = self.vram as *mut u8;
        for (dy, row) in CURSOR_SHAPE.iter().enumerate() {
            let y = self.cursor_y + dy as i32;
            if y < lo as i32 || y >= hi as i32 {
                continue;
            }
            for (dx, c) in row.bytes().enumerate() {
                let x = self.cursor_x + dx as i32;
                let color = match c {
                    b'#' => palette::CURSOR_BLACK,
                    b'o' => palette::CURSOR_WHITE,
                    _ => continue,
                };
                if (0..WIDTH as i32).contains(&x) {
                    // SAFETY: x, y は画面の中
                    unsafe { vram.add(y as usize * WIDTH + x as usize).write_volatile(color) };
                }
            }
        }
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
