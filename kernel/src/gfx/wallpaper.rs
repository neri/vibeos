//! デスクトップの背景の画像（BMP を読んで 32 ビットにし、並べ方と一緒に gui に渡す。並べるのは gui が 1 行ずつ）
//!
//! BOOT.CFG の `wallpaper=PATH [tile|center]` で起動時に、シェルの `wallpaper` でその場で変える。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

use vibe_gfx::bmp::Bmp;
use vibe_gfx::wallpaper::{self, Fit};

use crate::gui::{self, SCREEN};

/// 画像を読んで背景にする
pub fn load(path: &str, fit: Fit) -> Result<(), String> {
    if SCREEN.lock().mode() != gui::Mode::Desktop {
        return Err(String::from("only on the desktop"));
    }
    let data = crate::fs::read_file(path).map_err(|e| format!("{e}"))?;
    let bmp = Bmp::parse(&data).map_err(|e| format!("{e}"))?;
    if bmp.width * bmp.height > vibe_abi::WALLPAPER_MAX_PIXELS {
        return Err(String::from("too large"));
    }
    let (w, h) = (bmp.width as usize, bmp.height as usize);
    // 16 色の画面では読むときに 1 回だけ誤差拡散する（並べるときは色番号を写すだけ）
    let (pixels, pixels8) = if gui::is_classic() {
        (Box::default(), bmp.to_vga16(&vibe_gfx::dither::Lut16::new()).into_boxed_slice())
    } else {
        (bmp.to_rgb32().into_boxed_slice(), Box::default())
    };
    let mut s = SCREEN.lock();
    s.set_wallpaper(Some(gui::Wallpaper { w, h, fit, pixels, pixels8 }));
    s.present();
    Ok(())
}

/// 背景をグラデーションに戻す
pub fn clear() {
    let mut s = SCREEN.lock();
    s.set_wallpaper(None);
    s.present();
}

/// BOOT.CFG に `wallpaper=` があれば読む（デスクトップにした後に呼ぶ）
pub fn load_from_config() {
    let text = crate::fs::read_file("/BOOT.CFG").unwrap_or_default();
    let Some(value) = wallpaper::from_config(core::str::from_utf8(&text).unwrap_or("")) else { return };
    match wallpaper::parse_setting(value) {
        Some((path, fit)) => match load(path, fit) {
            Ok(()) => klog!("gui: wallpaper {path}"),
            Err(e) => klog!("gui: wallpaper {path}: {e}"),
        },
        None => klog!("gui: cannot read wallpaper={value}"),
    }
}
