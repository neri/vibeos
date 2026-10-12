//! 画面のハードウェア（VGA モード 13h・8 ビット、または VESA の 32 ビットのリニアフレームバッファ）
//!
//! 画面の大きさ・1 ピクセルのビット数・1 行のバイト数・フレームバッファの場所はローダが決めて
//! ブート情報で渡す。ここは「1 行を VRAM に書く」「パレットを設定する」だけを受け持ち、何をどこに
//! 描くかは `gui`（ウィンドウの重ね合わせ）が決める。

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::port::{inb, outb};
use crate::bootinfo::BootInfo;

/// 対応する一番大きな画面（ローダの MAX_SCREEN_W / MAX_SCREEN_H と同じ）
pub const MAX_WIDTH: usize = 2560;
pub const MAX_HEIGHT: usize = 1600;

const DAC_WRITE_INDEX: u16 = 0x3C8;
const DAC_DATA: u16 = 0x3C9;
const INPUT_STATUS_1: u16 = 0x3DA;
const VRETRACE: u8 = 0x08;

// 画面の情報（起動時に決まり、フレームバッファの番地だけページングの後に変わる）
static WIDTH: AtomicUsize = AtomicUsize::new(320);
static HEIGHT: AtomicUsize = AtomicUsize::new(200);
static PITCH: AtomicUsize = AtomicUsize::new(320);
static BPP: AtomicUsize = AtomicUsize::new(8);
static VRAM: AtomicUsize = AtomicUsize::new(0);

pub fn width() -> usize {
    WIDTH.load(Ordering::Relaxed)
}

pub fn height() -> usize {
    HEIGHT.load(Ordering::Relaxed)
}

/// 1 ピクセルのビット数（8 か 32）
pub fn bpp() -> usize {
    BPP.load(Ordering::Relaxed)
}

/// ページングを有効にする前に呼ぶ（フレームバッファは物理アドレスのまま触る）
pub fn init(info: &BootInfo) {
    let w = (info.screen_w as usize).min(MAX_WIDTH);
    let bpp = if info.bpp == 32 { 32 } else { 8 };
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store((info.screen_h as usize).min(MAX_HEIGHT), Ordering::Relaxed);
    BPP.store(bpp, Ordering::Relaxed);
    PITCH.store((info.pitch as usize).max(w * bpp / 8), Ordering::Relaxed);
    VRAM.store(info.vram_addr as usize, Ordering::Relaxed);
}

/// フレームバッファの物理アドレスと大きさ（ページングで対応付けるため）
pub fn framebuffer() -> (usize, usize) {
    (VRAM.load(Ordering::Relaxed), PITCH.load(Ordering::Relaxed) * height())
}

/// ページングを有効にした後、フレームバッファを対応付けた仮想アドレスに切り替える
pub fn set_framebuffer_address(virt: usize) {
    VRAM.store(virt, Ordering::Relaxed);
}

/// 画面の `y` 行目に `row`（画面の幅、4 の倍数バイト）を書く（8 ビットの画面）
pub fn write_row(y: usize, row: &[u8]) {
    let vram = VRAM.load(Ordering::Relaxed);
    if vram == 0 || y >= height() || bpp() != 8 {
        return;
    }
    let dst = (vram + y * PITCH.load(Ordering::Relaxed)) as *mut u32;
    let words = row.len().min(width()) / 4;
    for i in 0..words {
        let v = u32::from_le_bytes(row[i * 4..i * 4 + 4].try_into().unwrap());
        // SAFETY: VRAM の 1 行は pitch >= width バイト。y は画面の中
        unsafe { dst.add(i).write_volatile(v) };
    }
}

/// 画面の `y` 行目に `row`（画面の幅、0x00RRGGBB）を書く（32 ビットの画面）
pub fn write_row32(y: usize, row: &[u32]) {
    let vram = VRAM.load(Ordering::Relaxed);
    if vram == 0 || y >= height() || bpp() != 32 {
        return;
    }
    let dst = (vram + y * PITCH.load(Ordering::Relaxed)) as *mut u32;
    for (i, &v) in row.iter().take(width()).enumerate() {
        // SAFETY: VRAM の 1 行は pitch >= width * 4 バイト。y は画面の中
        unsafe { dst.add(i).write_volatile(v) };
    }
}

/// パレット全体を設定する（各 6bit）
pub fn set_palette(p: &[[u8; 3]; 256]) {
    set_palette_range(0, p);
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
