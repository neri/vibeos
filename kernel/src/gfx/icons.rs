//! アイコン（`/SYSTEM/ICONS.VIA` の 16x16 と、ドック用の `/SYSTEM/ICONS32.VIA` の 32x32。gzip のこともある。
//! 事前に掛けたアルファの ARGB）と、モード 12h 用の 16 色の 16x16（`/SYSTEM/ICONS4.VIA`。1 画素 1 バイトの色番号）
//!
//! 起動時に 1 回読み込み、ずっと持っておく。無ければアイコンを描かない（32x32 が無ければドックは 16x16 を使う）。

use core::sync::atomic::{AtomicPtr, Ordering};

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_gfx::icons::IconSet;

const PATH: &str = "/SYSTEM/ICONS.VIA";
const PATH32: &str = "/SYSTEM/ICONS32.VIA";
const PATH16: &str = "/SYSTEM/ICONS4.VIA";
/// アイコンの 1 辺（ふつうのものとドックのもの）
pub const SIZE: usize = 16;
pub const SIZE32: usize = 32;
/// 対応するアイコンが無いアプリのアイコン
const GENERIC: &str = "app";

type Icons = BTreeMap<String, Box<[u32]>>;
static ICONS: AtomicPtr<Icons> = AtomicPtr::new(core::ptr::null_mut());
static ICONS32: AtomicPtr<Icons> = AtomicPtr::new(core::ptr::null_mut());
/// 16 色のアイコン（色番号、透明は `vibe_gfx::icons::TRANSPARENT`）
type Icons16 = BTreeMap<String, Box<[u8]>>;
static ICONS16: AtomicPtr<Icons16> = AtomicPtr::new(core::ptr::null_mut());

/// アイコンを読み込む（ファイルシステムの準備の後に 1 回）
pub fn load() {
    load_set(PATH, SIZE, &ICONS);
    load_set(PATH32, SIZE32, &ICONS32);
    load_set16();
}

/// ファイルを読む（gzip なら展開する）
fn read(path: &str) -> Option<Vec<u8>> {
    use vibe_pack::gzip;
    let Ok(data) = crate::fs::read_file(path) else {
        klog!("icons: {path} is not available");
        return None;
    };
    if !gzip::is_gzip(&data) {
        return Some(data);
    }
    gzip::gunzip(&data, 1024 * 1024).map_err(|_| klog!("icons: {path} is broken")).ok()
}

fn load_set16() {
    let Some(data) = read(PATH16) else { return };
    let Ok(set) = IconSet::parse(&data).map_err(|_| klog!("icons: {PATH16} is broken")) else { return };
    if set.size != SIZE || !set.is_vga16() {
        return klog!("icons: {PATH16} is not 16-color {SIZE}x{SIZE}");
    }
    let mut map = Icons16::new();
    for name in set.names() {
        if let Some(px) = set.get16(name) {
            map.insert(String::from(name), Box::from(px));
        }
    }
    klog!("icons: {} icons (16 colors)", map.len());
    ICONS16.store(Box::leak(Box::new(map)), Ordering::Release);
}

fn load_set(path: &str, size: usize, slot: &AtomicPtr<Icons>) {
    let Some(data) = read(path) else { return };
    let Ok(set) = IconSet::parse(&data).map_err(|_| klog!("icons: {path} is broken")) else { return };
    if set.size != size {
        return klog!("icons: {path}: size {} is not {size}", set.size);
    }
    let mut map = Icons::new();
    for name in set.names() {
        if let Some(px) = set.get(name) {
            map.insert(String::from(name), px.into_boxed_slice());
        }
    }
    klog!("icons: {} icons ({size}x{size})", map.len());
    slot.store(Box::leak(Box::new(map)), Ordering::Release);
}

fn lookup(slot: &AtomicPtr<Icons>, name: &str) -> Option<&'static [u32]> {
    let p = slot.load(Ordering::Acquire);
    // SAFETY: 入れるのは Box::leak したものだけで、消さない
    let map: &'static Icons = if p.is_null() { return None } else { unsafe { &*p } };
    map.get(name).map(|b| &**b)
}

/// ドック用の 32x32 のアイコン（無ければ None）
pub fn get32(name: &str) -> Option<&'static [u32]> {
    lookup(&ICONS32, name)
}

/// 名前 `name` のアイコン
pub fn get(name: &str) -> Option<&'static [u32]> {
    lookup(&ICONS, name)
}

/// アプリ `name`（小文字、拡張子なし）のアイコン。無ければ汎用のもの
pub fn for_app(name: &str) -> Option<&'static [u32]> {
    get(name).or_else(|| get(GENERIC))
}

/// 名前 `name` の 16 色のアイコン（16x16 の色番号。モード 12h の画面用。使い始めるのは docs/plans/vga16.md の C3 から）
#[allow(dead_code)]
pub fn get16(name: &str) -> Option<&'static [u8]> {
    let p = ICONS16.load(Ordering::Acquire);
    // SAFETY: 入れるのは Box::leak したものだけで、消さない
    let map: &'static Icons16 = if p.is_null() { return None } else { unsafe { &*p } };
    map.get(name).map(|b| &**b)
}
