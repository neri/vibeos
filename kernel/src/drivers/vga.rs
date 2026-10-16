//! 画面のハードウェア（VGA モード 13h・8 ビット、VGA モード 12h・16 色の 4 枚の面、または VESA の
//! 32 ビットのリニアフレームバッファ）
//!
//! 画面の大きさ・1 ピクセルのビット数・1 行のバイト数・フレームバッファの場所はローダが決めて
//! ブート情報で渡す。ここは「1 行を VRAM に書く」「パレットを設定する」だけを受け持ち、何をどこに
//! 描くかは `gui`（ウィンドウの重ね合わせ）が決める。

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use vibe_font::Font;
use vibe_gfx::{palette, planar};

use crate::arch::port::{inb, outb};
use crate::bootinfo::BootInfo;
use crate::sync::BootCell;

/// 対応する一番大きな画面（ローダの MAX_SCREEN_W / MAX_SCREEN_H と同じ）
pub const MAX_WIDTH: usize = 2560;
pub const MAX_HEIGHT: usize = 1600;

const DAC_WRITE_INDEX: u16 = 0x3C8;
const DAC_DATA: u16 = 0x3C9;
const INPUT_STATUS_1: u16 = 0x3DA;
const VRETRACE: u8 = 0x08;
const ATTR: u16 = 0x3C0;
const SEQ_INDEX: u16 = 0x3C4;
const SEQ_DATA: u16 = 0x3C5;
const GC_INDEX: u16 = 0x3CE;
const GC_DATA: u16 = 0x3CF;
/// シーケンサのマップマスク（書き込む面）
const SEQ_MAP_MASK: u8 = 2;

// 画面の情報（起動時に決まり、フレームバッファの番地だけページングの後に変わる）
static WIDTH: AtomicUsize = AtomicUsize::new(320);
static HEIGHT: AtomicUsize = AtomicUsize::new(200);
static PITCH: AtomicUsize = AtomicUsize::new(320);
static BPP: AtomicUsize = AtomicUsize::new(8);
static VRAM: AtomicUsize = AtomicUsize::new(0);

/// VGA の BIOS の 8x16 のフォントの写し（ローダが低いメモリに置いたものを、ページングの前に写す）
static BIOS_FONT: BootCell<[u8; 256 * 16]> = BootCell::new([0; 256 * 16]);
static BIOS_FONT_OK: AtomicBool = AtomicBool::new(false);
/// BIOS_FONT を使う `Font`（使えるときだけ、起動直後に作る）
static FONT16: BootCell<Option<Font>> = BootCell::new(None);

pub fn width() -> usize {
    WIDTH.load(Ordering::Relaxed)
}

pub fn height() -> usize {
    HEIGHT.load(Ordering::Relaxed)
}

/// 1 ピクセルのビット数（8・32、モード 12h は 4）
pub fn bpp() -> usize {
    BPP.load(Ordering::Relaxed)
}

/// ページングを有効にする前に呼ぶ（フレームバッファは物理アドレスのまま触る）
pub fn init(info: &BootInfo) {
    let w = (info.screen_w as usize).min(MAX_WIDTH);
    let bpp = match info.bpp {
        32 => 32,
        4 => 4,
        _ => 8,
    };
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store((info.screen_h as usize).min(MAX_HEIGHT), Ordering::Relaxed);
    BPP.store(bpp, Ordering::Relaxed);
    // モード 12h の pitch は 1 枚の面の 1 行のバイト数（1 バイトに 8 ピクセル）
    let min_pitch = if bpp == 4 { w / 8 } else { w * bpp / 8 };
    PITCH.store((info.pitch as usize).max(min_pitch), Ordering::Relaxed);
    VRAM.store(info.vram_addr as usize, Ordering::Relaxed);
    if info.font16 != 0 {
        copy_bios_font(info.font16 as usize);
    }
    if bpp == 4 {
        init_planar();
    }
}

fn copy_bios_font(addr: usize) {
    // SAFETY: 起動直後で、ほかに誰も BIOS_FONT を触らない。addr はローダが 4096 バイトを写した所で、
    // まだページングが無いので物理アドレスのまま読める
    let font = unsafe { &mut *BIOS_FONT.as_ptr() };
    for (i, b) in font.iter_mut().enumerate() {
        // SAFETY: 同上
        *b = unsafe { ((addr + i) as *const u8).read_volatile() };
    }
    // 'A' に絵が無ければ使えないものとみなす
    if font[b'A' as usize * 16..][..16].iter().any(|&b| b != 0) {
        // SAFETY: 同上。BIOS_FONT はこの後書き換えないので &'static で渡してよい
        unsafe { *FONT16.as_ptr() = Some(Font::vga16(&*BIOS_FONT.as_ptr())) };
        BIOS_FONT_OK.store(true, Ordering::Release);
    }
}

/// 8x16 のフォント: VGA の BIOS のもの。無ければ 8x8 を縦に 2 倍したもの（16 色のデスクトップの字）
pub fn font16() -> &'static Font {
    if BIOS_FONT_OK.load(Ordering::Acquire) {
        // SAFETY: FONT16 は init（起動直後）の後は書き換えない
        if let Some(f) = unsafe { &*FONT16.as_ptr() } {
            return f;
        }
    }
    &vibe_font::FONT_8X16_TALL
}

/// VGA の BIOS の 8x16 のフォントのバイト列（アプリに渡す。256 字 × 16 バイト）
pub fn bios_font_bytes() -> Option<&'static [u8]> {
    // SAFETY: BIOS_FONT は init（起動直後）の後は書き換えない
    has_bios_font().then(|| unsafe { (&*BIOS_FONT.as_ptr()).as_slice() })
}

/// VGA の BIOS の 8x16 のフォントを使えるか
pub fn has_bios_font() -> bool {
    BIOS_FONT_OK.load(Ordering::Acquire)
}

/// モード 12h の書き込みの設定とパレット。BIOS の既定の値は当てにせず、全部書く
fn init_planar() {
    // SAFETY: VGA のレジスタに書くだけ（画面の色と書き込み方が決まる）
    unsafe {
        // グラフィックスコントローラ: Set/Reset 無効・論理演算なし・書き込みモード 0・ビットマスク全部
        for (index, value) in [(0u8, 0u8), (1, 0), (3, 0), (5, 0), (8, 0xFF)] {
            outb(GC_INDEX, index);
            outb(GC_DATA, value);
        }
        outb(SEQ_INDEX, SEQ_MAP_MASK);
        outb(SEQ_DATA, 0x0F);
        // アトリビュートコントローラのパレットを 0〜15 に並べ直す（BIOS の既定は 6 と 8〜15 を DAC の
        // 0x14・0x38〜0x3F に振っている）。0x3DA を読むと番号を書く順に戻る。番号の bit5 が 0 の間は
        // 表示が止まるので、最後に 0x20 を書いて戻す
        inb(INPUT_STATUS_1);
        for i in 0..16u8 {
            outb(ATTR, i);
            outb(ATTR, i);
        }
        // 色の選択（DAC の番号の上位）を 0 に
        outb(ATTR, 0x14);
        outb(ATTR, 0);
        outb(ATTR, 0x20);
    }
    set_dac(0, &palette::VGA16);
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

/// 画面の `y` 行目に `row`（画面の幅、色番号 0〜15）を書く（モード 12h）。4 枚の面に分け、面ごとに
/// マップマスクを切り替えて書く。VRAM は読まない
pub fn write_row4(y: usize, row: &[u8]) {
    write_row4_span(y, row, 0, row.len());
}

/// `write_row4` の、x が [x0, x1) の所だけ（8 ピクセル単位に広げる）を書く。`row` は画面の幅の 1 行全部
pub fn write_row4_span(y: usize, row: &[u8], x0: usize, x1: usize) {
    let vram = VRAM.load(Ordering::Relaxed);
    if vram == 0 || y >= height() || bpp() != 4 {
        return;
    }
    let row = &row[..row.len().min(width())];
    let (b0, b1) = (x0 / 8, x1.min(row.len()).div_ceil(8));
    if b0 >= b1 {
        return;
    }
    let mut packed = [0u32; MAX_WIDTH / 8];
    let n = planar::pack_row(&row[b0 * 8..(b1 * 8).min(row.len())], &mut packed);
    let dst = (vram + y * PITCH.load(Ordering::Relaxed) + b0) as *mut u8;
    for plane in 0..4 {
        // SAFETY: マップマスクを変えると、続く書き込みがその面にだけ入る
        unsafe {
            outb(SEQ_INDEX, SEQ_MAP_MASK);
            outb(SEQ_DATA, 1 << plane);
        }
        for (i, &p) in packed[..n].iter().enumerate() {
            // SAFETY: b0 + i < width / 8 <= pitch（1 枚の面の 1 行のバイト数）。y は画面の中
            unsafe { dst.add(i).write_volatile(planar::plane_byte(p, plane)) };
        }
    }
    // SAFETY: 全部の面に書く状態に戻す
    unsafe {
        outb(SEQ_INDEX, SEQ_MAP_MASK);
        outb(SEQ_DATA, 0x0F);
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

/// パレット全体を設定する（各 6bit）。モード 12h は 16 色の表（`palette::VGA16`）のまま変えないので何もしない
/// （色番号は画面に送るときに 16 色に直す）
pub fn set_palette(p: &[[u8; 3]; 256]) {
    set_palette_range(0, p);
}

/// パレットの一部を設定する（各 6bit）。モード 12h では何もしない
pub fn set_palette_range(start: u8, colors: &[[u8; 3]]) {
    if bpp() == 4 {
        return;
    }
    set_dac(start, colors);
}

fn set_dac(start: u8, colors: &[[u8; 3]]) {
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
