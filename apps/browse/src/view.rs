//! 描き方: レイアウトした文書を本文の枠に描く。フォントの寸法（`Metrics`）もここ
//!
//! 文字は 8x16 の固定幅（VGA の BIOS のフォント）だけ。太字と見出しは重ね打ちで、見出しは行を高くする。
//! 画面は白黒（docs/plans/mono.md）: 白い地に黒い文字、リンクは黒い字に下線（乗せると黒い地に白い字）。

use vibe_rt::font::Font as Bitmap;
use vibe_rt::gfx_lib::Surface;
use vibe_rt::gfx_lib::palette::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};
use vibe_ui::Rect;
use vibe_web::layout::{Font, Layout, Metrics, PieceKind};

pub const PAGE_BG: u8 = WHITE;
/// 画像の透明な所に重ねる色（`PAGE_BG` の 0x00RRGGBB）
pub const PAGE_BG_RGB: u32 = 0x00FF_FFFF;
pub const TEXT: u8 = BLACK;
pub const LINK: u8 = BLACK;
/// 乗せたリンク: 黒い地に白い字
pub const LINK_HOVER: u8 = WHITE;
const LINK_HOVER_BG: u8 = BLACK;
const RULE: u8 = DARK_GRAY;
const RULE_LIGHT: u8 = LIGHT_GRAY;
const IMAGE_FRAME: u8 = LIGHT_GRAY;
/// 8x16 のフォントの、字の上端から基準線までの高さ
const ASCENT: i32 = 12;
/// 見出しの行の高さと基準線（上下に余白を足す）
const HEADING_H: i32 = 22;
const HEADING_ASCENT: i32 = 15;

/// 文字のフォント（8x16 の 1 つだけ）
pub struct Fonts {
    font: &'static Bitmap,
}

impl Fonts {
    pub fn load() -> Fonts {
        Fonts { font: vibe_rt::gfx::font16() }
    }

    /// 文字を描く（`top` は行の上端）
    pub fn draw(&self, s: &mut Surface, f: Font, x: i32, top: i32, text: &str, color: u8) {
        match f {
            Font::Bold | Font::Heading => {
                s.draw_text_bold(self.font, x, top, text, color);
            }
            Font::Normal | Font::Mono => {
                s.draw_text(self.font, x, top, text, color, None);
            }
        }
    }
}

impl Metrics for Fonts {
    fn text_width(&self, _font: Font, s: &str) -> i32 {
        (s.chars().count() * self.font.width) as i32
    }

    fn line_height(&self, font: Font) -> i32 {
        if font == Font::Heading { HEADING_H } else { self.font.height as i32 }
    }

    fn ascent(&self, font: Font) -> i32 {
        if font == Font::Heading { HEADING_ASCENT } else { ASCENT }
    }
}

/// 文書を `area` に描く（文書の y = `scroll` が枠の上端）。`hover` のリンクは色を変える。
/// `image` は画像の番号から画素（白黒の色番号, 幅, 高さ）。無ければ枠だけ
pub fn draw_page<'a>(
    s: &mut Surface,
    fonts: &Fonts,
    layout: &Layout,
    area: Rect,
    scroll: i32,
    hover: Option<u32>,
    image: &dyn Fn(u32) -> Option<(&'a [u8], usize, usize)>,
) {
    s.fill_rect(area.x, area.y, area.w, area.h, PAGE_BG);
    for line in &layout.lines[layout.first_visible(scroll)..] {
        let ly = area.y + line.y - scroll;
        if ly >= area.y + area.h {
            break;
        }
        for p in &line.pieces {
            let x = area.x + p.x;
            match &p.kind {
                PieceKind::Text { text, font, link } => {
                    // 見出しも字の形は同じなので、基準線は 8x16 の字の基準線にそろえる
                    let top = ly + line.baseline - ASCENT;
                    let color = match link {
                        Some(l) if Some(*l) == hover => {
                            s.fill_rect(x, top, p.w, 16, LINK_HOVER_BG);
                            LINK_HOVER
                        }
                        Some(_) => LINK,
                        None => TEXT,
                    };
                    fonts.draw(s, *font, x, top, text, color);
                    if link.is_some() {
                        s.hline(x, ly + line.baseline + 2, p.w, color);
                    }
                }
                PieceKind::Bullet(level) => {
                    let cy = ly + line.baseline - 5;
                    match level {
                        0 => s.fill_circle(x + 3, cy, 3, TEXT),
                        1 => s.circle(x + 3, cy, 3, TEXT),
                        _ => s.fill_rect(x + 1, cy - 2, 5, 5, TEXT),
                    }
                }
                PieceKind::Rule => {
                    s.hline(x, ly, p.w, RULE);
                    s.hline(x, ly + 1, p.w, RULE_LIGHT);
                }
                PieceKind::Image { index, link } => {
                    let top = ly + line.baseline - p.h;
                    match image(*index) {
                        Some((px, w, h)) => draw_scaled(s, x, top, p.w, p.h, px, w, h),
                        None => s.rect(x, top, p.w, p.h, IMAGE_FRAME),
                    }
                    if link.is_some() {
                        // 乗せた画像のリンクは 2 ピクセルの枠
                        s.rect(x - 1, top - 1, p.w + 2, p.h + 2, LINK);
                        if *link == hover {
                            s.rect(x - 2, top - 2, p.w + 4, p.h + 4, LINK);
                        }
                    }
                }
            }
        }
    }
}

/// 画像を w×h に縮めて（最も近い画素で）描く
#[allow(clippy::too_many_arguments)]
fn draw_scaled(s: &mut Surface, x: i32, y: i32, w: i32, h: i32, src: &[u8], sw: usize, sh: usize) {
    if w as usize == sw && h as usize == sh {
        s.blit(x, y, src, sw, None);
        return;
    }
    for dy in 0..h {
        let sy = (dy as usize * sh) / h.max(1) as usize;
        for dx in 0..w {
            let sx = (dx as usize * sw) / w.max(1) as usize;
            if let Some(&px) = src.get(sy * sw + sx) {
                s.put_pixel(x + dx, y + dy, px);
            }
        }
    }
}
