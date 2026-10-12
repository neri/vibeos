//! なめらかなフォント（`/SYSTEM/FONTS/*.VFN`。gzip のこともある）
//!
//! 起動時に 1 回読み込み、ずっと持っておく（`&'static`）。無い・壊れていれば None で、
//! 使う側は 8x8 のビットマップのフォントで描く。

use core::sync::atomic::{AtomicPtr, Ordering};

use alloc::boxed::Box;
use alloc::vec::Vec;

use vibe_font::VFont;

const DIR: &str = "/SYSTEM/FONTS";
/// 展開した 1 つのフォントの大きさの上限（全角の字を足しても足りるように）
const MAX_SIZE: usize = 4 * 1024 * 1024;

static UI: AtomicPtr<VFont<'static>> = AtomicPtr::new(core::ptr::null_mut());
static UI_BOLD: AtomicPtr<VFont<'static>> = AtomicPtr::new(core::ptr::null_mut());
static MONO: AtomicPtr<VFont<'static>> = AtomicPtr::new(core::ptr::null_mut());

/// フォントを読み込む（ファイルシステムの準備の後、デスクトップにする前に 1 回呼ぶ）
pub fn load() {
    for (name, slot) in [("UI.VFN", &UI), ("UIBOLD.VFN", &UI_BOLD), ("MONO.VFN", &MONO)] {
        match read(name) {
            Some(font) => {
                klog!("fonts: {name} {}px, {} glyphs", font.size, font.glyph_count());
                slot.store(Box::leak(Box::new(font)), Ordering::Release);
            }
            None => klog!("fonts: {name} is not available"),
        }
    }
}

fn read(name: &str) -> Option<VFont<'static>> {
    use vibe_pack::gzip;
    let data = crate::fs::read_file(&alloc::format!("{DIR}/{name}")).ok()?;
    let data: Vec<u8> = if gzip::is_gzip(&data) { gzip::gunzip(&data, MAX_SIZE).ok()? } else { data };
    // ずっと使うので手放さない
    let data: &'static [u8] = Box::leak(data.into_boxed_slice());
    VFont::parse(data).ok()
}

fn get(slot: &AtomicPtr<VFont<'static>>) -> Option<&'static VFont<'static>> {
    let p = slot.load(Ordering::Acquire);
    // SAFETY: 入れるのは Box::leak したものだけで、消さない
    (!p.is_null()).then(|| unsafe { &*p })
}

/// UI の文字（プロポーショナル）
pub fn ui() -> Option<&'static VFont<'static>> {
    get(&UI)
}

/// UI の太字（タイトル）。無ければふつうの UI の文字
pub fn ui_bold() -> Option<&'static VFont<'static>> {
    get(&UI_BOLD).or_else(ui)
}

/// 端末の文字（固定幅）
pub fn mono() -> Option<&'static VFont<'static>> {
    get(&MONO).filter(|f| f.cell_width > 0)
}
