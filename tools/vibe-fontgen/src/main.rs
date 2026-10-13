//! vibe-fontgen: TrueType のフォントを決まった大きさでラスタライズして `.VFN`（crates/vibe-font の vfn）にする。
//!
//! 使い方:
//!   vibe-fontgen TTF SIZE OUT              プロポーショナル（行の高さとベースラインはフォントから）。
//!                                          SIZE は em の大きさ（ピクセル。CSS の font-size と同じ）
//!   vibe-fontgen TTF SIZE OUT CELLWxCELLH BASELINE
//!                                          固定幅（端末用）: 1 文字の枠と、枠の上端からのベースライン
//!
//! 字は ASCII の表示できる文字（0x20〜0x7E）。濃さは 4 ビット（16 段階）にし、小さな文字が細く薄く
//! 見えないよう少し濃くする（覆う割合の 0.8 乗）。ラスタライズは ab_glyph（ヒンティングはしない）。
//! 毎回同じ出力になる。

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use vibe_font::vfn::{SUBPIXELS, build::{GlyphData, encode}};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 && args.len() != 5 {
        eprintln!("usage: vibe-fontgen TTF SIZE OUT [CELLWxCELLH BASELINE]");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[0]).unwrap_or_else(|e| panic!("{}: {e}", args[0]));
    let font = FontRef::try_from_slice(&data).expect("not a TrueType font");
    let size: u16 = args[1].parse().expect("bad size");
    let mono = (args.len() == 5).then(|| {
        let (w, h) = args[3].split_once('x').expect("expected CELLWxCELLH");
        let parse = |s: &str| s.parse::<u16>().expect("bad number");
        (parse(w), parse(h), parse(&args[4]))
    });
    // ab_glyph の PxScale は「ascent − descent」の高さなので、em の大きさから直す
    let em = font.units_per_em().expect("no units per em");
    let scale = PxScale::from(size as f32 * (font.ascent_unscaled() - font.descent_unscaled()) / em);
    let sf = font.as_scaled(scale);
    let (line_height, ascent, cell_width) = match mono {
        Some((w, h, base)) => (h, base, w),
        None => ((sf.ascent() - sf.descent()).ceil() as u16, sf.ascent().round() as u16, 0),
    };
    let mut glyphs: Vec<GlyphData> = (0x20u32..=0x7E).map(|code| rasterize(&font, scale, code, cell_width)).collect();
    let out = encode(size, line_height, ascent, cell_width, &mut glyphs);
    std::fs::write(&args[2], &out).unwrap_or_else(|e| panic!("{}: {e}", args[2]));
    eprintln!("{}: {} glyphs, line {line_height}, baseline {ascent}, {} bytes", args[2], glyphs.len(), out.len());
}

/// 1 文字をペンの位置 (0, 0)（ベースライン）でラスタライズする
fn rasterize(font: &FontRef, scale: PxScale, code: u32, cell_width: u16) -> GlyphData {
    let c = char::from_u32(code).unwrap();
    let id = font.glyph_id(c);
    let advance = if cell_width > 0 {
        cell_width * SUBPIXELS as u16
    } else {
        (font.as_scaled(scale).h_advance(id) * SUBPIXELS as f32).round() as u16
    };
    let empty = GlyphData { code, width: 0, height: 0, left: 0, top: 0, advance, alpha: Vec::new() };
    let Some(outline) = font.outline_glyph(id.with_scale_and_position(scale, point(0.0, 0.0))) else {
        return empty;
    };
    let b = outline.px_bounds();
    let (w, h) = (b.width() as usize, b.height() as usize);
    if w == 0 || h == 0 {
        return empty;
    }
    let mut alpha = vec![0u8; w * h];
    outline.draw(|x, y, cov| {
        let v = cov.clamp(0.0, 1.0).powf(0.8);
        alpha[y as usize * w + x as usize] = (v * 15.0).round() as u8;
    });
    GlyphData {
        code,
        width: w as u8,
        height: h as u8,
        left: b.min.x as i8,
        top: b.min.y as i8,
        advance,
        alpha,
    }
}
