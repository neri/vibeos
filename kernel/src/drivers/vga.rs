//! 画面（VGA モード 13h または VESA 640x480・8 ビット）と「画面（ディスプレイ）」の切り替え
//!
//! - 画面の大きさ・1 行のバイト数・フレームバッファの場所はローダが決めてブート情報で渡す
//! - 画面は複数持てる。0 番はコンソール（ステータスバー + 文字）で画面と同じ大きさ（静的な領域）。
//!   アプリの画面は 320x200（`APP_WIDTH` x `APP_HEIGHT`）で、画面が 640x480 のときは 2 倍にして
//!   真ん中に出す（320x200 用に書いたアプリがそのまま動くように）
//! - 描画はそれぞれの画面の裏バッファに行い、`present` で前面の画面の変更した行だけ VRAM に転送する
//! - マウスカーソルは転送するときに VRAM の上にだけ描く（裏バッファは汚さない）

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use vibe_gfx::{Surface, palette};

use crate::arch::port::{inb, outb};
use crate::bootinfo::BootInfo;
use crate::sync::IrqMutex;

/// 対応する一番大きな画面
pub const MAX_WIDTH: usize = 640;
pub const MAX_HEIGHT: usize = 480;
/// アプリの画面（gfx_open）の大きさ
pub const APP_WIDTH: usize = 320;
pub const APP_HEIGHT: usize = 200;

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

/// 画面の大きさ（ロックを取らずに読めるように別に持つ）
static SCREEN_W: AtomicUsize = AtomicUsize::new(320);
static SCREEN_H: AtomicUsize = AtomicUsize::new(200);

pub fn width() -> usize {
    SCREEN_W.load(Ordering::Relaxed)
}

pub fn height() -> usize {
    SCREEN_H.load(Ordering::Relaxed)
}

/// アプリの画面を出すときの (倍率, 左上の x, 左上の y)
pub fn app_placement() -> (usize, usize, usize) {
    let (w, h) = (width(), height());
    if w >= APP_WIDTH * 2 && h >= APP_HEIGHT * 2 {
        (2, (w - APP_WIDTH * 2) / 2, (h - APP_HEIGHT * 2) / 2)
    } else {
        (1, 0, 0)
    }
}

/// 画面上の位置をアプリの画面の上の位置にする（はみ出した分は端に寄せる）
pub fn screen_to_app(x: i32, y: i32) -> (i32, i32) {
    let (scale, x0, y0) = app_placement();
    let ax = ((x - x0 as i32) / scale as i32).clamp(0, APP_WIDTH as i32 - 1);
    let ay = ((y - y0 as i32) / scale as i32).clamp(0, APP_HEIGHT as i32 - 1);
    (ax, ay)
}

#[repr(C, align(16))]
pub struct Screen {
    console: [u8; MAX_WIDTH * MAX_HEIGHT],
    apps: BTreeMap<DisplayId, Box<[u8]>>,
    next_id: DisplayId,
    foreground: DisplayId,
    width: usize,
    height: usize,
    /// VRAM の 1 行のバイト数
    pitch: usize,
    vram: usize,
    /// VRAM に未転送の行の範囲 [dirty_lo, dirty_hi)（画面の行）
    dirty_lo: usize,
    dirty_hi: usize,
    cursor_x: i32,
    cursor_y: i32,
    cursor_visible: bool,
}

/// 画面。CONSOLE と両方取るときは CONSOLE を先に取ること
///
/// 初期値はすべて 0 にしておく（0 でない値があると大きなバッファごと .data に入り、カーネルの
/// ファイルが大きくなる）。0 でない初期値は `init` で設定する。
pub static SCREEN: IrqMutex<Screen> = IrqMutex::new(Screen {
    console: [0; MAX_WIDTH * MAX_HEIGHT],
    apps: BTreeMap::new(),
    next_id: 0,
    foreground: CONSOLE,
    width: 0,
    height: 0,
    pitch: 0,
    vram: 0,
    dirty_lo: 0,
    dirty_hi: 0,
    cursor_x: 0,
    cursor_y: 0,
    cursor_visible: false,
});

/// ページングを有効にする前に呼ぶ（フレームバッファは物理アドレスのまま触る）
pub fn init(info: &BootInfo) {
    let mut s = SCREEN.lock();
    s.width = (info.screen_w as usize).min(MAX_WIDTH);
    s.height = (info.screen_h as usize).min(MAX_HEIGHT);
    s.pitch = info.pitch.max(info.screen_w as u32) as usize;
    s.vram = info.vram_addr as usize;
    SCREEN_W.store(s.width, Ordering::Relaxed);
    SCREEN_H.store(s.height, Ordering::Relaxed);
    s.cursor_x = (s.width / 2) as i32;
    s.cursor_y = (s.height / 2) as i32;
    set_palette(&palette::standard());
    s.surface().clear(palette::BLACK);
    let h = s.height;
    s.mark_dirty(0, h);
    s.present();
}

/// フレームバッファの物理アドレスと大きさ（ページングで対応付けるため）
pub fn framebuffer() -> (usize, usize) {
    let s = SCREEN.lock();
    (s.vram, s.pitch * s.height)
}

/// ページングを有効にした後、フレームバッファを対応付けた仮想アドレスに切り替える
pub fn set_framebuffer_address(virt: usize) {
    SCREEN.lock().vram = virt;
}

impl Screen {
    /// コンソールの画面
    pub fn surface(&mut self) -> Surface<'_> {
        let (w, h) = (self.width, self.height);
        Surface::new(&mut self.console[..w * h], w, h)
    }

    /// コンソールの画面の行 [y0, y1) を書き換えたことを記録する
    pub fn mark_dirty(&mut self, y0: usize, y1: usize) {
        self.mark_display_dirty(CONSOLE, y0, y1);
    }

    /// アプリ用の画面（320x200）を作る（まだ前面には出さない）
    pub fn create_display(&mut self) -> DisplayId {
        self.next_id += 1; // 0 はコンソールなので 1 から
        let id = self.next_id;
        // スタックに置かないよう、ヒープに直接作る
        self.apps.insert(id, vec![palette::BLACK; APP_WIDTH * APP_HEIGHT].into_boxed_slice());
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
        self.apps.get_mut(&id).map(|b| Surface::new(b, APP_WIDTH, APP_HEIGHT))
    }

    /// 画面 `id` の行 [y0, y1)（その画面の座標）を書き換えたことを記録する
    pub fn mark_display_dirty(&mut self, id: DisplayId, y0: usize, y1: usize) {
        if id != self.foreground {
            return;
        }
        if id == CONSOLE {
            self.mark_rows(y0, y1);
        } else {
            let (scale, _, top) = app_placement();
            self.mark_rows(top + y0 * scale, top + y1 * scale);
        }
    }

    pub fn set_foreground(&mut self, id: DisplayId) {
        if id == CONSOLE || self.apps.contains_key(&id) {
            self.foreground = id;
            let h = self.height;
            self.mark_rows(0, h);
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
        let y1 = y1.min(self.height);
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
        let words = self.width / 4;
        for y in lo..hi {
            let dst = (self.vram + y * self.pitch) as *mut u32;
            match self.apps.get(&self.foreground) {
                None => {
                    let src = self.console[y * self.width..].as_ptr() as *const u32;
                    for i in 0..words {
                        // SAFETY: コンソールのバッファは width*height バイト、VRAM の行は pitch >= width バイト
                        unsafe { dst.add(i).write_volatile(src.add(i).read_unaligned()) };
                    }
                }
                Some(app) => Self::present_app_row(app, y, dst, words),
            }
        }
        if self.cursor_visible {
            self.draw_cursor(lo, hi);
        }
        self.dirty_lo = 0;
        self.dirty_hi = 0;
    }

    /// アプリの画面（320x200）の、画面の y 行目に当たる部分を書く（2 倍のときは横も縦も 2 倍）
    fn present_app_row(app: &[u8], y: usize, dst: *mut u32, words: usize) {
        let (scale, x0, y0) = app_placement();
        let row = (y as isize - y0 as isize).div_euclid(scale as isize);
        for i in 0..words {
            let x = i * 4;
            let mut px = [0u8; 4];
            if (0..APP_HEIGHT as isize).contains(&row) {
                for (k, p) in px.iter_mut().enumerate() {
                    let sx = (x + k) as isize - x0 as isize;
                    if sx >= 0 && (sx as usize) < APP_WIDTH * scale {
                        *p = app[row as usize * APP_WIDTH + sx as usize / scale];
                    }
                }
            }
            // SAFETY: VRAM の行は pitch >= width バイト
            unsafe { dst.add(i).write_volatile(u32::from_le_bytes(px)) };
        }
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
                if (0..self.width as i32).contains(&x) {
                    // SAFETY: x, y は画面の中
                    unsafe { vram.add(y as usize * self.pitch + x as usize).write_volatile(color) };
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

/// パレットの一部を設定する（各 6bit）
pub fn set_palette_range(start: u8, colors: &[[u8; 3]]) {
    // SAFETY: DAC への書き込みは色を変えるだけ
    unsafe {
        outb(DAC_WRITE_INDEX, start);
        for c in colors {
            outb(DAC_DATA, c[0] & 0x3F);
            outb(DAC_DATA, c[1] & 0x3F);
            outb(DAC_DATA, c[2] & 0x3F);
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
