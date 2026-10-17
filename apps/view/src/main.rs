//! 画像ビューア: BMP を画像の大きさのウィンドウに表示し、キーか [x] で閉じる。
//! 色は VGA の 16 色に誤差拡散する（8 ビットの画像も。`Bmp::to_vga16`）

#![no_std]
#![no_main]

use vibe_rt::gfx::{Event, Window};
use vibe_rt::gfx_lib::bmp::Bmp;
use vibe_rt::gfx_lib::dither::Lut16;
use vibe_rt::println;

vibe_rt::entry!(main);

fn main() -> i32 {
    let Some(path) = vibe_rt::args().nth(1) else {
        println!("usage: view file.bmp");
        return 2;
    };
    let data = match vibe_rt::read_file(path) {
        Ok(d) => d,
        Err(e) => {
            println!("view: {path}: {}", vibe_rt::abi::err::name(e));
            return 1;
        }
    };
    let bmp = match Bmp::parse(&data) {
        Ok(b) => b,
        Err(e) => {
            println!("view: {path}: {e}");
            return 1;
        }
    };
    println!("view: showing {}x{}", bmp.width, bmp.height);
    let kind = if bmp.is_indexed() { "8-bit" } else { "true color" };
    if !show_window(path, &bmp) {
        println!("view: cannot open a window");
        return 1;
    }
    println!("view: closed ({}x{} {kind})", bmp.width, bmp.height);
    0
}

/// 画像の大きさのウィンドウに 16 色に誤差拡散して出し、キーか [x] を待つ（画面に入りきらなければ左上の部分）。
/// ウィンドウを開けなければ false
fn show_window(path: &str, bmp: &Bmp) -> bool {
    let info = vibe_rt::sys::sys_info();
    let (w, h) = (bmp.width as usize, bmp.height as usize);
    // 枠とタイトルバー・上端のバーの分を残す（win_open は 640x480 まで）
    let (ww, wh) = (w.min(info.screen_width as usize - 8).min(640), h.min(info.screen_height as usize - 60).min(480));
    let title = path.rsplit('/').next().unwrap_or(path);
    let Some(mut win) = Window::open(ww.max(1), wh.max(1), title) else { return false };
    let pixels = bmp.to_vga16(&Lut16::new());
    win.surface().blit(0, 0, &pixels, w, None);
    win.present();
    loop {
        match win.event(None) {
            Some(Event::Close) | None => return true,
            Some(Event::Key(k)) if k.pressed() => return true,
            _ => {}
        }
    }
}
