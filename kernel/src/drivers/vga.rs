//! 画面のハードウェア（VGA のモード 12h: 640x480・4 枚の面。docs/plans/vga16.md・docs/plans/mono.md）
//!
//! モードの設定はローダ（`int 10h AX=0012h`）が済ませ、画面の大きさ・1 枚の面の 1 行のバイト数・VRAM の場所を
//! ブート情報で渡す。ここは「1 行を白黒にして VRAM に書く」「書き込み方とパレットを決める」だけを
//! 受け持ち、何をどこに描くかは `gui`（ウィンドウの重ね合わせ）が決める。
//! 画素は白（15）と黒（0）だけ: 4 枚の面に同じバイトを 1 回で書く（マップマスクは 0x0F のまま）。
//! VGA の BIOS の 8x16 のフォント（ローダが写したもの）もここで持つ。

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use vibe_font::Font;
use vibe_gfx::{mono, palette};

use crate::arch::port::{inb, outb};
use crate::bootinfo::BootInfo;
use crate::sync::BootCell;

/// 画面の大きさ（モード 12h）。ブート情報の値がこれと違っても、この大きさまでしか使わない
const MAX_WIDTH: usize = 640;
const MAX_HEIGHT: usize = 480;

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
/// 1 行のバイト数（1 バイトに 8 ピクセル）
const ROW_BYTES: usize = MAX_WIDTH / 8;

// 画面の情報（起動時に決まる）
static WIDTH: AtomicUsize = AtomicUsize::new(MAX_WIDTH);
static HEIGHT: AtomicUsize = AtomicUsize::new(MAX_HEIGHT);
/// 1 枚の面の 1 行のバイト数（1 バイトに 8 ピクセル）
static PITCH: AtomicUsize = AtomicUsize::new(MAX_WIDTH / 8);
static VRAM: AtomicUsize = AtomicUsize::new(0);

/// VRAM に今入っているバイトの写し（1 行 ROW_BYTES バイト × MAX_HEIGHT 行。4 枚の面はどれも同じ）。
/// 同じバイトは書かない（ISA の VGA への書き込みは遅いため）。`init_planar` で VRAM を 0 にしてから使う
static SHADOW: BootCell<[u8; ROW_BYTES * MAX_HEIGHT]> = BootCell::new([0; ROW_BYTES * MAX_HEIGHT]);
/// 書いたバイトと、同じだったので書かなかったバイトの数（ktest で効き目を見る）
static WRITTEN: AtomicUsize = AtomicUsize::new(0);
static SKIPPED: AtomicUsize = AtomicUsize::new(0);

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

/// ページングを有効にする前に呼ぶ。VRAM（0xA0000〜）は同じ番地に対応付けた RAM の範囲の中にある
pub fn init(info: &BootInfo) {
    let w = (info.screen_w as usize).min(MAX_WIDTH);
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store((info.screen_h as usize).min(MAX_HEIGHT), Ordering::Relaxed);
    PITCH.store((info.pitch as usize).max(w / 8), Ordering::Relaxed);
    VRAM.store(info.vram_addr as usize, Ordering::Relaxed);
    if info.font16 != 0 {
        copy_bios_font(info.font16 as usize);
    }
    init_planar();
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

/// 8x16 のフォント: VGA の BIOS のもの。無ければ 8x8 を縦に 2 倍したもの（デスクトップの字）
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
        // DAC の 0〜15 を 16 色の表に（各 6bit）。この後は変えない
        outb(DAC_WRITE_INDEX, 0);
        for c in palette::VGA16 {
            outb(DAC_DATA, c[0] & 0x3F);
            outb(DAC_DATA, c[1] & 0x3F);
            outb(DAC_DATA, c[2] & 0x3F);
        }
    }
    // 4 枚の面を全部 0 にして、写し（0 で始まる）と合わせる（BIOS が消していることを当てにしない）。
    // マップマスクは 0x0F のまま（この後の書き込みも 4 枚の面に同時に入る）
    let vram = VRAM.load(Ordering::Relaxed);
    if vram != 0 {
        let len = PITCH.load(Ordering::Relaxed) * height();
        for i in 0..len {
            // SAFETY: マップマスクは 0x0F（4 枚の面に同時に書く）。i は VRAM の 1 枚の面の大きさの中
            unsafe { ((vram + i) as *mut u8).write_volatile(0) };
        }
    }
}

/// 書いたバイトと、写しと同じだったので書かなかったバイトの数（起動してからの合計）
#[allow(dead_code)]
pub fn write_stats() -> (usize, usize) {
    (WRITTEN.load(Ordering::Relaxed), SKIPPED.load(Ordering::Relaxed))
}

/// 画面の `y` 行目の、x が [x0, x1) の所（8 ピクセル単位に広げる）を書く。`row` は画面の幅の 1 行全部の色番号。
/// 色番号を白黒の模様にして（`mono`）、4 枚の面に同じバイトを書く。VRAM は読まず（ISA では遅い）、今入っている
/// バイトの写し（`SHADOW`）と同じバイトは書かない。呼ぶのは画面のロック（`gui::SCREEN`）の中か、パニックのときだけ
pub fn write_row_mono(y: usize, row: &[u8], x0: usize, x1: usize) {
    let vram = VRAM.load(Ordering::Relaxed);
    if vram == 0 || y >= height() {
        return;
    }
    let row = &row[..row.len().min(width())];
    let (b0, b1) = (x0 / 8, x1.min(row.len()).div_ceil(8));
    if b0 >= b1 {
        return;
    }
    let mut packed = [0u8; ROW_BYTES];
    let n = mono::pack_row(&row[b0 * 8..(b1 * 8).min(row.len())], y, &mut packed);
    let dst = (vram + y * PITCH.load(Ordering::Relaxed) + b0) as *mut u8;
    // SAFETY: SHADOW は画面のロックの中（とパニックのとき）でだけ触る
    let shadow = unsafe { &mut *SHADOW.as_ptr() };
    let line = &mut shadow[y * ROW_BYTES + b0..][..n];
    let mut written = 0;
    for (i, &b) in packed[..n].iter().enumerate() {
        if line[i] == b {
            continue;
        }
        line[i] = b;
        written += 1;
        // SAFETY: マップマスクは 0x0F（4 枚の面に同時に書く）。b0 + i < width / 8 <= pitch。y は画面の中
        unsafe { dst.add(i).write_volatile(b) };
    }
    WRITTEN.fetch_add(written, Ordering::Relaxed);
    SKIPPED.fetch_add(n - written, Ordering::Relaxed);
}

/// VRAM の写しの `y` 行目（ktest で、書いたバイトが白黒の模様と一致するかを見る）
#[cfg(feature = "qemu-test")]
pub fn shadow_row(y: usize) -> [u8; ROW_BYTES] {
    // SAFETY: SHADOW は画面のロックの中でだけ書く。呼ぶのは画面のロックを持っているときだけ
    let shadow = unsafe { &*SHADOW.as_ptr() };
    let mut r = [0u8; ROW_BYTES];
    r.copy_from_slice(&shadow[y * ROW_BYTES..][..ROW_BYTES]);
    r
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
