//! デスクトップの背景の画像（BMP を読んで、画面の大きさに並べて gui に渡す）
//!
//! BOOT.CFG の `wallpaper=PATH [tile|center]` で起動時に、シェルの `wallpaper` でその場で変える。

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
    let pixels = bmp.to_standard_palette();
    let (sw, sh) = SCREEN.lock().wallpaper_size();
    let (iw, ih) = (bmp.width as usize, bmp.height as usize);
    let out = wallpaper::compose(&pixels, iw, ih, sw, sh, fit, gui::DESKTOP_BG);
    let mut s = SCREEN.lock();
    s.set_wallpaper(Some(out.into_boxed_slice()));
    s.present();
    Ok(())
}

/// 背景を単色に戻す
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
