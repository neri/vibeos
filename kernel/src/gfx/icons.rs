//! アイコン（`/SYSTEM/ICONS.VIA`。白黒の 16x16、1 画素 1 バイトの色番号（0 か 15）で透明は 0xFF。gzip のこともある）
//!
//! 起動時に 1 回読み込み、ずっと持っておく。無ければアイコンを描かない。

use core::sync::atomic::{AtomicPtr, Ordering};

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use vibe_gfx::icons::IconSet;

const PATH: &str = "/SYSTEM/ICONS.VIA";
/// アイコンの 1 辺
pub const SIZE: usize = 16;
/// 対応するアイコンが無いアプリのアイコン
const GENERIC: &str = "app";

type Icons = BTreeMap<String, Box<[u8]>>;
static ICONS: AtomicPtr<Icons> = AtomicPtr::new(core::ptr::null_mut());

/// アイコンを読み込む（ファイルシステムの準備の後に 1 回）
pub fn load() {
    use vibe_pack::gzip;
    let Ok(data) = crate::fs::read_file(PATH) else {
        return klog!("icons: {PATH} is not available");
    };
    let data: Vec<u8> = if gzip::is_gzip(&data) {
        match gzip::gunzip(&data, 1024 * 1024) {
            Ok(d) => d,
            Err(_) => return klog!("icons: {PATH} is broken"),
        }
    } else {
        data
    };
    let Ok(set) = IconSet::parse(&data).map_err(|_| klog!("icons: {PATH} is broken")) else { return };
    if set.size != SIZE {
        return klog!("icons: {PATH}: size {} is not {SIZE}", set.size);
    }
    let mut map = Icons::new();
    for name in set.names() {
        if let Some(px) = set.get(name) {
            map.insert(String::from(name), Box::from(px));
        }
    }
    klog!("icons: {} icons", map.len());
    ICONS.store(Box::leak(Box::new(map)), Ordering::Release);
}

/// 名前 `name` のアイコン（16x16 の色番号）
pub fn get(name: &str) -> Option<&'static [u8]> {
    let p = ICONS.load(Ordering::Acquire);
    // SAFETY: 入れるのは Box::leak したものだけで、消さない
    let map: &'static Icons = if p.is_null() { return None } else { unsafe { &*p } };
    map.get(name).map(|b| &**b)
}

/// アプリ `name`（小文字、拡張子なし）のアイコン。無ければ汎用のもの
pub fn for_app(name: &str) -> Option<&'static [u8]> {
    get(name).or_else(|| get(GENERIC))
}
