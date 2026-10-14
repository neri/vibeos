mod doc;
mod fuzz;
mod html;
mod http;
mod layout;
mod url;

use crate::layout::{Font, Layout, Metrics, PieceKind};

/// 1 文字 8 ピクセル（見出しは 12）、行の高さ 16（見出しは 24）の偽のフォント
pub struct Fixed;

impl Metrics for Fixed {
    fn text_width(&self, font: Font, s: &str) -> i32 {
        let w = if font == Font::Heading { 12 } else { 8 };
        s.chars().count() as i32 * w
    }

    fn line_height(&self, font: Font) -> i32 {
        if font == Font::Heading { 24 } else { 16 }
    }

    fn ascent(&self, font: Font) -> i32 {
        if font == Font::Heading { 18 } else { 12 }
    }
}

/// 各行の文字（部品の間は空白 1 つ。点は `*`、水平線は `---`、画像は `<img N>`）
pub fn lines_text(l: &Layout) -> Vec<String> {
    l.lines
        .iter()
        .map(|line| {
            line.pieces
                .iter()
                .map(|p| match &p.kind {
                    PieceKind::Text { text, .. } => text.clone(),
                    PieceKind::Bullet(_) => "*".into(),
                    PieceKind::Rule => "---".into(),
                    PieceKind::Image { index, .. } => format!("<img {index}>"),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

pub fn lay(html: &str, width: i32) -> Layout {
    crate::layout::layout(&crate::doc::parse(html), width, &Fixed, &|_| None)
}
