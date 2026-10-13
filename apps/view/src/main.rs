//! 画像ビューア: BMP を表示し、キーを押すと戻る。デスクトップでは画像の大きさのウィンドウ（[x] でも閉じる）、
//! 320x200 では全画面
//!
//! デスクトップでは 32 ビットのウィンドウに画像の色をそのまま出す（24/32 ビットも 8 ビットも正確な色）。
//! 320x200 では、8 ビットの画像は画像のパレットをそのまま使い、24/32 ビットは標準パレットに誤差拡散で変換する。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_rt::gfx::{Event, HEIGHT, Screen, WIDTH, Window32};
use vibe_rt::gfx_lib::bmp::Bmp;
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
    if vibe_rt::gfx::is_desktop() {
        // デスクトップでは画像の大きさの 32 ビットのウィンドウ（画面に入りきらなければ左上の部分）
        if !show_window(path, &bmp) {
            println!("view: cannot open a window");
            return 1;
        }
        println!("view: closed ({}x{} {kind})", bmp.width, bmp.height);
        return 0;
    }
    let (pixels, palette): (Vec<u8>, Option<Vec<[u8; 3]>>) = if bmp.is_indexed() {
        let px = (0..bmp.height).flat_map(|y| (0..bmp.width).map(move |x| (x, y))).map(|(x, y)| bmp.index(x, y));
        (px.collect(), Some(bmp.colors().iter().map(|c| [c[0] >> 2, c[1] >> 2, c[2] >> 2]).collect()))
    } else {
        (bmp.to_standard_palette(), None)
    };

    let Some(mut screen) = Screen::open(false) else {
        println!("view: cannot open the screen");
        return 1;
    };
    if let Some(p) = &palette {
        screen.set_palette(0, p);
    }
    let x = (WIDTH as i32 - bmp.width as i32).max(0) / 2;
    let y = (HEIGHT as i32 - bmp.height as i32).max(0) / 2;
    screen.surface().blit(x, y, &pixels, bmp.width as usize, None);
    screen.present();

    vibe_rt::wait_key();
    drop(screen);
    println!("view: closed ({}x{} {kind})", bmp.width, bmp.height);
    0
}

/// 画像の大きさの 32 ビットのウィンドウに出して、キーか [x] を待つ。ウィンドウを開けなければ false
fn show_window(path: &str, bmp: &Bmp) -> bool {
    let info = vibe_rt::sys::sys_info();
    let (w, h) = (bmp.width as usize, bmp.height as usize);
    // 枠とタイトルバー・上端のバーの分を残す（win_open は 640x480 まで）
    let (ww, wh) = (w.min(info.screen_width as usize - 8).min(640), h.min(info.screen_height as usize - 60).min(480));
    let title = path.rsplit('/').next().unwrap_or(path);
    let Some(mut win) = Window32::open(ww.max(1), wh.max(1), title) else { return false };
    let mut c = win.canvas();
    for y in 0..wh {
        for x in 0..ww {
            let [r, g, b] = bmp.rgb(x as u32, y as u32);
            c.put_pixel(x as i32, y as i32, vibe_rt::gfx_lib::canvas::rgb(r, g, b));
        }
    }
    win.present();
    loop {
        match win.event(None) {
            Some(Event::Close) | None => return true,
            Some(Event::Key(k)) if k.pressed() => return true,
            _ => {}
        }
    }
}
