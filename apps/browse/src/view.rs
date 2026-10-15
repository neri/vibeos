//! 描き方: レイアウトした文書を本文の枠に描く。フォントの寸法（`Metrics`）もここ

use vibe_rt::font::{FONT_8X8, VFont};
use vibe_rt::gfx_lib::canvas::{Canvas, Color, Corner, rgb};
use vibe_rt::gfx_lib::theme;
use vibe_ui::Rect;
use vibe_web::layout::{Font, Layout, Metrics, PieceKind};

pub const PAGE_BG: Color = theme::FACE;
pub const TEXT: Color = theme::TEXT;
pub const LINK: Color = theme::ACCENT;
pub const LINK_HOVER: Color = theme::ACCENT_HOVER;
const RULE: Color = rgb(200, 198, 190);
const RULE_LIGHT: Color = rgb(232, 230, 223);
const IMAGE_FRAME: Color = theme::HOVER;

/// カーネルから写したフォント。無いものは代わり（見出しは太字、ほかは 8x8）
pub struct Fonts {
    normal: Option<&'static VFont<'static>>,
    bold: Option<&'static VFont<'static>>,
    mono: Option<&'static VFont<'static>>,
    heading: Option<&'static VFont<'static>>,
}

impl Fonts {
    pub fn load() -> Fonts {
        use vibe_rt::abi::{FONT_BOLD, FONT_HEADING, FONT_MONO, FONT_UI};
        use vibe_rt::gfx::font;
        let normal = font(FONT_UI);
        let bold = font(FONT_BOLD).or(normal);
        Fonts { normal, bold, mono: font(FONT_MONO).or(normal), heading: font(FONT_HEADING).or(bold) }
    }

    fn get(&self, f: Font) -> Option<&'static VFont<'static>> {
        match f {
            Font::Normal => self.normal,
            Font::Bold => self.bold,
            Font::Mono => self.mono,
            Font::Heading => self.heading,
        }
    }

    /// 文字を描く（`top` は行の上端）
    pub fn draw(&self, c: &mut Canvas, f: Font, x: i32, top: i32, s: &str, color: Color) {
        match self.get(f) {
            Some(v) => {
                c.draw_vtext(v, x, top, s, color);
            }
            None => {
                c.draw_text(&FONT_8X8, x, top + 1, s, color, None);
            }
        }
    }
}

impl Metrics for Fonts {
    fn text_width(&self, font: Font, s: &str) -> i32 {
        match self.get(font) {
            Some(v) => v.measure(s),
            None => (s.chars().count() * FONT_8X8.width) as i32,
        }
    }

    fn line_height(&self, font: Font) -> i32 {
        self.get(font).map_or(10, |v| v.line_height as i32)
    }

    fn ascent(&self, font: Font) -> i32 {
        self.get(font).map_or(8, |v| v.ascent as i32)
    }
}

/// 文書を `area` に描く（文書の y = `scroll` が枠の上端）。`hover` のリンクは色を変える。
/// `image` は画像の番号から画素（0x00RRGGBB, 幅, 高さ）。無ければ枠だけ
pub fn draw_page<'a>(
    c: &mut Canvas,
    fonts: &Fonts,
    layout: &Layout,
    area: Rect,
    scroll: i32,
    hover: Option<u32>,
    image: &dyn Fn(u32) -> Option<(&'a [u32], usize, usize)>,
) {
    c.fill_rect(area.x, area.y, area.w, area.h, PAGE_BG);
    let dot = Corner::new(3);
    for line in &layout.lines[layout.first_visible(scroll)..] {
        let ly = area.y + line.y - scroll;
        if ly >= area.y + area.h {
            break;
        }
        for p in &line.pieces {
            let x = area.x + p.x;
            match &p.kind {
                PieceKind::Text { text, font, link } => {
                    let top = ly + line.baseline - fonts.ascent(*font);
                    let color = match link {
                        Some(l) if Some(*l) == hover => LINK_HOVER,
                        Some(_) => LINK,
                        None => TEXT,
                    };
                    fonts.draw(c, *font, x, top, text, color);
                    if link.is_some() {
                        c.hline(x, ly + line.baseline + 2, p.w, color);
                    }
                }
                PieceKind::Bullet(level) => {
                    let cy = ly + line.baseline - fonts.ascent(Font::Normal) * 2 / 5 - 3;
                    match level {
                        0 => c.fill_round_rect(x, cy, 6, 6, &dot, TEXT, 255),
                        1 => c.round_rect(x, cy, 6, 6, &dot, TEXT),
                        _ => c.fill_rect(x, cy, 5, 5, TEXT),
                    }
                }
                PieceKind::Rule => {
                    c.hline(x, ly, p.w, RULE);
                    c.hline(x, ly + 1, p.w, RULE_LIGHT);
                }
                PieceKind::Image { index, link } => {
                    let top = ly + line.baseline - p.h;
                    match image(*index) {
                        Some((px, w, h)) => draw_scaled(c, x, top, p.w, p.h, px, w, h),
                        None => c.rect(x, top, p.w, p.h, IMAGE_FRAME),
                    }
                    if link.is_some() {
                        let color = if *link == hover { LINK_HOVER } else { LINK };
                        c.rect(x - 1, top - 1, p.w + 2, p.h + 2, color);
                    }
                }
            }
        }
    }
}

/// 画像を w×h に縮めて（最も近い画素で）描く
#[allow(clippy::too_many_arguments)]
fn draw_scaled(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, src: &[u32], sw: usize, sh: usize) {
    if w as usize == sw && h as usize == sh {
        c.blit(x, y, src, sw);
        return;
    }
    for dy in 0..h {
        let sy = (dy as usize * sh) / h.max(1) as usize;
        for dx in 0..w {
            let sx = (dx as usize * sw) / w.max(1) as usize;
            if let Some(&px) = src.get(sy * sw + sx) {
                c.put_pixel(x + dx, y + dy, px);
            }
        }
    }
}
