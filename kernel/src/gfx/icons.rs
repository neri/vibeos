//! アイコン（`/SYSTEM/ICONS.VIA` の 16x16 と、ドック用の `/SYSTEM/ICONS32.VIA` の 32x32。gzip のこともある。
//! 事前に掛けたアルファの ARGB）
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
/// アイコンの 1 辺（ふつうのものとドックのもの）
pub const SIZE: usize = 16;
pub const SIZE32: usize = 32;
/// 対応するアイコンが無いアプリのアイコン
const GENERIC: &str = "app";

type Icons = BTreeMap<String, Box<[u32]>>;
static ICONS: AtomicPtr<Icons> = AtomicPtr::new(core::ptr::null_mut());
static ICONS32: AtomicPtr<Icons> = AtomicPtr::new(core::ptr::null_mut());

/// アイコンを読み込む（ファイルシステムの準備の後に 1 回）
pub fn load() {
    load_set(PATH, SIZE, &ICONS);
    load_set(PATH32, SIZE32, &ICONS32);
}

fn load_set(path: &str, size: usize, slot: &AtomicPtr<Icons>) {
    use vibe_pack::gzip;
    let Ok(data) = crate::fs::read_file(path) else {
        klog!("icons: {path} is not available");
        return;
    };
    let data: Vec<u8> = if gzip::is_gzip(&data) {
        match gzip::gunzip(&data, 1024 * 1024) {
            Ok(d) => d,
            Err(_) => return klog!("icons: {path} is broken"),
        }
    } else {
        data
    };
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
