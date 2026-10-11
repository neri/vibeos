//! 画像ビューア: BMP を全画面で表示し、キーを押すと戻る
//!
//! 8 ビットの画像は画像のパレットをそのまま VGA に設定して正確な色で表示する（終わったら戻す）。
//! 24/32 ビットの画像は標準パレットの近い色に置き換える。

use alloc::string::String;
use alloc::vec::Vec;

use vibe_gfx::bmp::Bmp;
use vibe_gfx::palette;

use crate::drivers::keyboard;
use crate::drivers::vga::{self, SCREEN};
use crate::fs;

pub fn run(path: &str) -> Result<String, String> {
    let data = fs::read_file(path).map_err(|e| alloc::format!("{path}: {e}"))?;
    let bmp = Bmp::parse(&data).map_err(|e| alloc::format!("{path}: {e}"))?;
    let (pixels, custom_palette) = if bmp.is_indexed() {
        let mut pal = [[0u8; 3]; 256];
        for (i, c) in bmp.colors().iter().enumerate() {
            pal[i] = [c[0] >> 2, c[1] >> 2, c[2] >> 2];
        }
        let px: Vec<u8> = (0..bmp.height).flat_map(|y| (0..bmp.width).map(move |x| (x, y))).map(|(x, y)| bmp.index(x, y)).collect();
        (px, Some(pal))
    } else {
        (bmp.to_standard_palette(), None)
    };

    let id = {
        let mut s = SCREEN.lock();
        let id = s.create_display();
        {
            let mut surf = s.display_surface(id).unwrap();
            // 画面より大きければ左上を表示し、小さければ真ん中に置く
            let x = (vga::WIDTH as i32 - bmp.width as i32).max(0) / 2;
            let y = (vga::HEIGHT as i32 - bmp.height as i32).max(0) / 2;
            surf.blit(x, y, &pixels, bmp.width as usize, None);
        }
        if let Some(p) = &custom_palette {
            vga::set_palette(p);
        }
        s.set_foreground(id);
        s.present();
        id
    };
    klog!("view: showing {}x{}", bmp.width, bmp.height);

    // 何かキーを押したら戻る
    while !keyboard::read_event().pressed {}

    {
        let mut s = SCREEN.lock();
        if custom_palette.is_some() {
            vga::set_palette(&palette::standard());
        }
        s.destroy_display(id);
        s.present();
    }
    let kind = if bmp.is_indexed() { "8-bit" } else { "true color" };
    Ok(alloc::format!("{}x{} {kind}", bmp.width, bmp.height))
}
