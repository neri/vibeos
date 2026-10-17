//! vibe-icongen: 手で書いた SVG のアイコンを 16x16 にラスタライズし、VGA の 16 色にして（アルファは半分で切り、
//! 色は一番近い 16 色。`vibe_gfx::icons::quantize16`）1 つのファイル（`.VIA`。crates/vibe-gfx の icons）にまとめる。
//!
//! 使い方: vibe-icongen OUT SVG...（アイコンの名前はファイル名から拡張子を除いたもの、最大 12 文字）
//!
//! SVG は自分で読む（外部のクレートを使わない）。対応するのはアイコンに使う一部だけ:
//! - `<svg viewBox="0 0 W H">`（正方形）
//! - `<rect x y width height rx>`・`<circle cx cy r>`・`<ellipse cx cy rx ry>`・`<polygon points>`・
//!   `<polyline points>`・`<line x1 y1 x2 y2>`・`<path d>`（M L H V Z と小文字。曲線は無し）
//! - 属性 `fill`（`#rrggbb`・`#rgb`・`none`）・`stroke`・`stroke-width`・`opacity`・`fill-opacity`・`stroke-opacity`
//! - `<g>` の属性は中の図形に引き継ぐ（fill・stroke・stroke-width・opacity）
//!
//! 1 ピクセルを 8×8 の点に分けて、図形を書いた順に塗り重ね、平均してアルファ付きの色にする。

use std::path::Path;

const SIZE: usize = 16;
const SS: usize = 8;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let size = SIZE;
    if args.len() < 2 {
        eprintln!("usage: vibe-icongen OUT SVG...");
        std::process::exit(2);
    }
    let mut icons = Vec::new();
    for path in &args[1..] {
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let pixels = render(&text, size).unwrap_or_else(|e| panic!("{path}: {e}"));
        icons.push((name, pixels));
    }
    icons.sort_by(|a, b| a.0.cmp(&b.0));
    let icons: Vec<(String, Vec<u8>)> = icons.iter().map(|(n, px)| (n.clone(), vibe_gfx::icons::quantize16(px))).collect();
    let out = vibe_gfx::icons::encode(size as u16, &icons);
    std::fs::write(&args[0], &out).unwrap_or_else(|e| panic!("{}: {e}", args[0]));
    eprintln!("{}: {} icons, {} bytes", args[0], icons.len(), out.len());
}

#[derive(Clone, Copy)]
struct Paint {
    rgb: [f64; 3],
    alpha: f64,
}

#[derive(Clone)]
struct Style {
    fill: Option<[f64; 3]>,
    stroke: Option<[f64; 3]>,
    stroke_width: f64,
    opacity: f64,
    fill_opacity: f64,
    stroke_opacity: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style { fill: Some([0.0; 3]), stroke: None, stroke_width: 1.0, opacity: 1.0, fill_opacity: 1.0, stroke_opacity: 1.0 }
    }
}

enum Shape {
    /// 塗る多角形（いくつかの輪。nonzero で中を決める）と、線を引く折れ線
    Poly { rings: Vec<Vec<(f64, f64)>>, closed: bool },
    RoundRect { x: f64, y: f64, w: f64, h: f64, r: f64 },
    Ellipse { cx: f64, cy: f64, rx: f64, ry: f64 },
}

struct Item {
    shape: Shape,
    style: Style,
}

fn render(text: &str, size: usize) -> Result<Vec<u32>, String> {
    let mut items = Vec::new();
    let mut view = 32.0;
    let mut stack: Vec<Style> = vec![Style::default()];
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find('>').ok_or("unclosed tag")?;
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            if name.trim() == "g" {
                stack.pop();
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attrs) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let attrs = parse_attrs(attrs);
        let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        let mut style = stack.last().cloned().unwrap_or_default();
        apply_style(&mut style, &attrs)?;
        let shape = match name {
            "svg" => {
                if let Some(vb) = get("viewBox") {
                    let v: Vec<f64> = vb.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                    if v.len() == 4 {
                        view = v[2];
                    }
                }
                continue;
            }
            "g" => {
                if !self_closing {
                    stack.push(style);
                }
                continue;
            }
            "rect" => Shape::RoundRect { x: num("x"), y: num("y"), w: num("width"), h: num("height"), r: num("rx") },
            "circle" => Shape::Ellipse { cx: num("cx"), cy: num("cy"), rx: num("r"), ry: num("r") },
            "ellipse" => Shape::Ellipse { cx: num("cx"), cy: num("cy"), rx: num("rx"), ry: num("ry") },
            "line" => Shape::Poly { rings: vec![vec![(num("x1"), num("y1")), (num("x2"), num("y2"))]], closed: false },
            "polygon" | "polyline" => {
                Shape::Poly { rings: vec![parse_points(get("points").unwrap_or(""))], closed: name == "polygon" }
            }
            "path" => {
                let (rings, closed) = parse_path(get("d").unwrap_or(""))?;
                Shape::Poly { rings, closed }
            }
            _ => continue,
        };
        items.push(Item { shape, style });
    }
    let scale = view / (size * SS) as f64;
    let n = size * SS;
    // 細かい点の色（事前に掛けたアルファ）
    let mut fine = vec![[0.0f64; 4]; n * n];
    for (i, px) in fine.iter_mut().enumerate() {
        let (x, y) = (((i % n) as f64 + 0.5) * scale, ((i / n) as f64 + 0.5) * scale);
        for it in &items {
            for (paint, inside) in [(fill_paint(&it.style), true), (stroke_paint(&it.style), false)] {
                let Some(p) = paint else { continue };
                let hit = if inside { inside_shape(&it.shape, x, y) } else { on_stroke(&it.shape, x, y, it.style.stroke_width / 2.0) };
                if hit {
                    let a = p.alpha;
                    for c in 0..3 {
                        px[c] = p.rgb[c] * a + px[c] * (1.0 - a);
                    }
                    px[3] = a + px[3] * (1.0 - a);
                }
            }
        }
    }
    let mut out = vec![0u32; size * size];
    for (i, o) in out.iter_mut().enumerate() {
        let (ox, oy) = (i % size, i / size);
        let mut acc = [0.0f64; 4];
        for sy in 0..SS {
            for sx in 0..SS {
                let p = fine[(oy * SS + sy) * n + ox * SS + sx];
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
        }
        let k = (SS * SS) as f64;
        let to8 = |v: f64| (v / k * 255.0).round().clamp(0.0, 255.0) as u32;
        *o = to8(acc[3]) << 24 | to8(acc[0]) << 16 | to8(acc[1]) << 8 | to8(acc[2]);
    }
    Ok(out)
}

fn fill_paint(s: &Style) -> Option<Paint> {
    s.fill.map(|rgb| Paint { rgb, alpha: s.opacity * s.fill_opacity })
}

fn stroke_paint(s: &Style) -> Option<Paint> {
    s.stroke.filter(|_| s.stroke_width > 0.0).map(|rgb| Paint { rgb, alpha: s.opacity * s.stroke_opacity })
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next() else { break };
        let after = &after[1..];
        let Some(close) = after.find(q) else { break };
        out.push((key, after[..close].to_string()));
        rest = after[close + 1..].trim_start();
    }
    out
}

fn parse_color(v: &str) -> Result<Option<[f64; 3]>, String> {
    let v = v.trim();
    if v == "none" {
        return Ok(None);
    }
    let hex = v.strip_prefix('#').ok_or_else(|| format!("unsupported color {v}"))?;
    let digits: Vec<u32> = hex.chars().map(|c| c.to_digit(16).ok_or_else(|| format!("bad color {v}"))).collect::<Result<_, _>>()?;
    let rgb = match digits.len() {
        3 => [digits[0] * 17, digits[1] * 17, digits[2] * 17],
        6 => [digits[0] * 16 + digits[1], digits[2] * 16 + digits[3], digits[4] * 16 + digits[5]],
        _ => return Err(format!("bad color {v}")),
    };
    Ok(Some(rgb.map(|c| c as f64 / 255.0)))
}

fn apply_style(s: &mut Style, attrs: &[(String, String)]) -> Result<(), String> {
    for (k, v) in attrs {
        let f = || v.trim().parse::<f64>().map_err(|_| format!("bad number {v}"));
        match k.as_str() {
            "fill" => s.fill = parse_color(v)?,
            "stroke" => s.stroke = parse_color(v)?,
            "stroke-width" => s.stroke_width = f()?,
            "opacity" => s.opacity *= f()?,
            "fill-opacity" => s.fill_opacity = f()?,
            "stroke-opacity" => s.stroke_opacity = f()?,
            _ => {}
        }
    }
    Ok(())
}

fn parse_points(s: &str) -> Vec<(f64, f64)> {
    let v: Vec<f64> = s.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|x| x.parse().ok()).collect();
    v.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

/// M L H V Z（大文字は絶対、小文字は相対）。輪のどれかが Z で閉じていれば閉じた図形とする
fn parse_path(d: &str) -> Result<(Vec<Vec<(f64, f64)>>, bool), String> {
    let mut toks = Vec::new();
    let mut num = String::new();
    for c in d.chars() {
        if c.is_ascii_alphabetic() {
            if !num.is_empty() {
                toks.push(std::mem::take(&mut num));
            }
            toks.push(c.to_string());
        } else if c == ',' || c.is_whitespace() || (c == '-' && !num.is_empty() && !num.ends_with('e')) {
            if !num.is_empty() {
                toks.push(std::mem::take(&mut num));
            }
            if c == '-' {
                num.push(c);
            }
        } else {
            num.push(c);
        }
    }
    if !num.is_empty() {
        toks.push(num);
    }
    let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    let mut cmd = 'M';
    let mut closed = false;
    let mut i = 0;
    let n = |i: &mut usize| -> Result<f64, String> {
        let t = toks.get(*i).ok_or("path ends early")?;
        *i += 1;
        t.parse().map_err(|_| format!("bad path number {t}"))
    };
    while i < toks.len() {
        if let Some(c) = toks[i].chars().next().filter(|c| c.is_ascii_alphabetic()) {
            cmd = c;
            i += 1;
            if c == 'Z' || c == 'z' {
                closed = true;
                x = sx;
                y = sy;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                x = bx + n(&mut i)?;
                y = by + n(&mut i)?;
                sx = x;
                sy = y;
                rings.push(vec![(x, y)]);
                cmd = if rel { 'l' } else { 'L' };
                continue;
            }
            'L' => {
                x = bx + n(&mut i)?;
                y = by + n(&mut i)?;
            }
            'H' => x = bx + n(&mut i)?,
            'V' => y = by + n(&mut i)?,
            c => return Err(format!("unsupported path command {c}")),
        }
        rings.last_mut().ok_or("path must start with M")?.push((x, y));
    }
    Ok((rings, closed))
}

fn inside_shape(s: &Shape, x: f64, y: f64) -> bool {
    match s {
        Shape::RoundRect { x: rx, y: ry, w, h, r } => {
            if x < *rx || y < *ry || x > rx + w || y > ry + h {
                return false;
            }
            let r = r.min(w / 2.0).min(h / 2.0);
            let cx = x.clamp(rx + r, rx + w - r);
            let cy = y.clamp(ry + r, ry + h - r);
            (x - cx).powi(2) + (y - cy).powi(2) <= r * r
        }
        Shape::Ellipse { cx, cy, rx, ry } => {
            *rx > 0.0 && *ry > 0.0 && ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0
        }
        Shape::Poly { rings, .. } => {
            // nonzero
            let mut wind = 0;
            for ring in rings {
                for k in 0..ring.len() {
                    let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                    if a.1 <= y {
                        if b.1 > y && cross(a, b, (x, y)) > 0.0 {
                            wind += 1;
                        }
                    } else if b.1 <= y && cross(a, b, (x, y)) < 0.0 {
                        wind -= 1;
                    }
                }
            }
            wind != 0
        }
    }
}

fn cross(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    (b.0 - a.0) * (p.1 - a.1) - (p.0 - a.0) * (b.1 - a.1)
}

fn seg_dist(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    ((a.0 + t * dx - p.0).powi(2) + (a.1 + t * dy - p.1).powi(2)).sqrt()
}

fn on_stroke(s: &Shape, x: f64, y: f64, half: f64) -> bool {
    match s {
        Shape::RoundRect { x: rx, y: ry, w, h, r } => {
            // 外側と内側の角の丸い矩形の間
            let outer = Shape::RoundRect { x: rx - half, y: ry - half, w: w + 2.0 * half, h: h + 2.0 * half, r: r + half };
            let inner = Shape::RoundRect { x: rx + half, y: ry + half, w: w - 2.0 * half, h: h - 2.0 * half, r: (r - half).max(0.0) };
            inside_shape(&outer, x, y) && !inside_shape(&inner, x, y)
        }
        Shape::Ellipse { cx, cy, rx, ry } => {
            let outer = Shape::Ellipse { cx: *cx, cy: *cy, rx: rx + half, ry: ry + half };
            let inner = Shape::Ellipse { cx: *cx, cy: *cy, rx: rx - half, ry: ry - half };
            inside_shape(&outer, x, y) && !inside_shape(&inner, x, y)
        }
        Shape::Poly { rings, closed } => rings.iter().any(|ring| {
            let n = ring.len();
            let segs = if *closed { n } else { n.saturating_sub(1) };
            (0..segs).any(|k| seg_dist(ring[k], ring[(k + 1) % n], (x, y)) <= half)
        }),
    }
}
