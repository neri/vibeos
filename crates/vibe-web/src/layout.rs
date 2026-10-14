//! 文書（`Document`）を決まった幅に並べる
//!
//! 結果は上から順の行（`Line`）で、行の中に文字・リストの点・水平線・画像の部品（`Piece`）が並ぶ。
//! 座標は文書の左上を (0, 0) にしたピクセル。文字の幅と高さは `Metrics` から取る（ホストのテストでは
//! 1 文字の幅を決め打ちにした偽物を使う）。
//!
//! - 空白（ASCII の空白）はまとめて 1 つにし、行の頭では捨てる。単語の切れ目で折り返し、1 行に入らない
//!   単語は文字の途中で切る。改行しない空白（U+00A0）は空白として描くが、そこでは折り返さない
//! - `Pre` の中は空白と改行をそのまま残し、タブは 8 文字ごとにそろえる。幅を越えたら文字の途中で折り返す
//! - 段落・見出し・`Pre`・引用・いちばん外のリスト・表の前後は `GAP` だけあける（続いた間は大きい方だけ）
//! - リストの項目は印を字下げの左に置く。表の行は 1 行ずつ、セルは左から間をあけて並べるだけ
//! - 画像は大きさが分かっていれば（`image_size`）その大きさの部品（幅に入らなければ縮める）、
//!   分からなければ `[alt]` の文字にする

use alloc::string::String;
use alloc::vec::Vec;

use crate::charset::NBSP;
use crate::doc::{Block, Document, Item, Marker, Style};

/// 文字の種類（どのフォントで描くか）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    Normal,
    Bold,
    Mono,
    /// 大きい見出し（h1・h2）
    Heading,
}

pub trait Metrics {
    /// 文字列の幅
    fn text_width(&self, font: Font, s: &str) -> i32;
    /// 行の高さ
    fn line_height(&self, font: Font) -> i32;
    /// 行の上端から文字の基準線まで
    fn ascent(&self, font: Font) -> i32;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PieceKind {
    Text {
        text: String,
        font: Font,
        link: Option<u32>,
    },
    /// リストの点（入れ子の深さ）
    Bullet(u8),
    Rule,
    Image {
        index: u32,
        link: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub x: i32,
    pub w: i32,
    /// 画像・水平線の高さ（文字では 0）
    pub h: i32,
    pub kind: PieceKind,
}

impl Piece {
    pub fn link(&self) -> Option<u32> {
        match self.kind {
            PieceKind::Text { link, .. } | PieceKind::Image { link, .. } => link,
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub y: i32,
    pub height: i32,
    /// 行の上端から基準線まで
    pub baseline: i32,
    pub pieces: Vec<Piece>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub lines: Vec<Line>,
    /// 印の名前と y
    pub anchors: Vec<(String, i32)>,
}

/// 文書のまわりの余白
pub const MARGIN: i32 = 8;
/// 引用・リスト・`dd` の字下げ
pub const INDENT: i32 = 28;
/// 表のセルの間
pub const CELL_GAP: i32 = 20;
/// リストの印と項目の文字の間
const MARKER_GAP: i32 = 6;

impl Layout {
    /// y を含む行の番号
    pub fn line_at(&self, y: i32) -> Option<usize> {
        let i = self.lines.partition_point(|l| l.y + l.height <= y);
        (i < self.lines.len() && self.lines[i].y <= y).then_some(i)
    }

    /// y が見える範囲 [top, top + h) にかかる最初の行
    pub fn first_visible(&self, top: i32) -> usize {
        self.lines.partition_point(|l| l.y + l.height <= top)
    }

    /// (x, y) にあるリンク
    pub fn link_at(&self, x: i32, y: i32) -> Option<u32> {
        let line = &self.lines[self.line_at(y)?];
        line.pieces.iter().find(|p| x >= p.x && x < p.x + p.w).and_then(Piece::link)
    }

    /// 印の y（無ければ None）
    pub fn anchor(&self, name: &str) -> Option<i32> {
        self.anchors.iter().find(|(n, _)| n == name).map(|&(_, y)| y)
    }
}

struct Ctx {
    block: Block,
    left: i32,
}

struct Builder<'a> {
    m: &'a dyn Metrics,
    width: i32,
    right: i32,
    left: i32,
    stack: Vec<Ctx>,
    lines: Vec<Line>,
    anchors: Vec<(String, i32)>,
    /// y の決まっていない印
    pending: Vec<String>,
    cur: Vec<Piece>,
    /// 次に置く位置
    x: i32,
    y: i32,
    /// 次の行の前にあける間
    gap: i32,
    /// 行に文字か画像を置いたか（印だけの行では false）
    started: bool,
    /// まとめた空白が前にある
    space: bool,
    /// `Pre` の行の何文字目か（タブをそろえるため）
    col: usize,
    /// `Pre` を開いた直後（すぐ後ろの改行は捨てる。HTML の決まり）
    pre_start: bool,
}

/// 文書を幅 `width` に並べる。`image_size` は画像の番号から分かっている大きさ
pub fn layout(doc: &Document, width: i32, m: &dyn Metrics, image_size: &dyn Fn(u32) -> Option<(u32, u32)>) -> Layout {
    let width = width.max(MARGIN * 2 + 40);
    let mut b = Builder {
        m,
        width,
        right: width - MARGIN,
        left: MARGIN,
        stack: Vec::new(),
        lines: Vec::new(),
        anchors: Vec::new(),
        pending: Vec::new(),
        cur: Vec::new(),
        x: MARGIN,
        y: MARGIN,
        gap: 0,
        started: false,
        space: false,
        col: 0,
        pre_start: false,
    };
    for item in &doc.items {
        match item {
            Item::Open(block) => b.open(*block),
            Item::Close => b.close(),
            Item::Text(s, style) => b.text(s, *style),
            Item::Break => b.line_break(),
            Item::Rule => b.rule(),
            Item::Image(i) => {
                let img = &doc.images[*i as usize];
                match image_size(*i) {
                    Some((w, h)) => b.image(*i, w as i32, h as i32, img.link),
                    None => {
                        let alt = if img.alt.is_empty() { "[image]" } else { img.alt.as_str() };
                        let s = if img.alt.is_empty() { String::from(alt) } else { ["[", alt, "]"].concat() };
                        let style = Style { link: img.link, ..Style::default() };
                        b.text(&s, style);
                    }
                }
            }
            // 次に終える行の y にする（その前に開くブロックの間が決まってから）
            Item::Anchor(name) => b.pending.push(name.clone()),
        }
    }
    b.break_line();
    let y = b.y;
    b.anchors.extend(b.pending.drain(..).map(|n| (n, y)));
    Layout { width: b.width, height: b.y + MARGIN, lines: b.lines, anchors: b.anchors }
}

fn font_of(style: Style) -> Font {
    if style.mono {
        Font::Mono
    } else if matches!(style.heading, 1 | 2) {
        Font::Heading
    } else if style.bold || style.heading > 0 {
        Font::Bold
    } else {
        Font::Normal
    }
}

impl Builder<'_> {
    fn gap_size(&self) -> i32 {
        self.m.line_height(Font::Normal) * 3 / 4
    }

    fn in_pre(&self) -> bool {
        self.stack.iter().any(|c| c.block == Block::Pre)
    }

    fn add_gap(&mut self, g: i32) {
        self.gap = self.gap.max(g);
    }

    /// 今の行を終える
    fn finish_line(&mut self) {
        let pieces = core::mem::take(&mut self.cur);
        let mut ascent = 0;
        let mut descent = 0;
        for p in &pieces {
            let (a, d) = match &p.kind {
                PieceKind::Text { font, .. } => {
                    let a = self.m.ascent(*font);
                    (a, self.m.line_height(*font) - a)
                }
                PieceKind::Bullet(_) => {
                    (self.m.ascent(Font::Normal), self.m.line_height(Font::Normal) - self.m.ascent(Font::Normal))
                }
                PieceKind::Rule => (p.h, 0),
                PieceKind::Image { .. } => (p.h, 2),
            };
            ascent = ascent.max(a);
            descent = descent.max(d);
        }
        if pieces.is_empty() {
            ascent = self.m.ascent(Font::Normal);
            descent = self.m.line_height(Font::Normal) - ascent;
        }
        if !self.lines.is_empty() {
            self.y += self.gap;
        }
        self.gap = 0;
        let height = ascent + descent;
        let y = self.y;
        self.anchors.extend(self.pending.drain(..).map(|n| (n, y)));
        self.lines.push(Line { y: self.y, height, baseline: ascent, pieces });
        self.y += height;
        self.x = self.left;
        self.started = false;
        self.space = false;
        self.col = 0;
    }

    /// 行に何かあれば終える
    fn break_line(&mut self) {
        if !self.cur.is_empty() {
            self.finish_line();
        }
        self.x = self.left;
        self.space = false;
        self.col = 0;
    }

    fn open(&mut self, block: Block) {
        let nested_list = block == Block::List && self.stack.iter().any(|c| matches!(c.block, Block::List));
        match block {
            Block::Para | Block::Heading(_) | Block::Pre | Block::Quote | Block::Table => {
                self.break_line();
                let g = match block {
                    Block::Heading(1 | 2) => self.gap_size() * 4 / 3,
                    _ => self.gap_size(),
                };
                self.add_gap(g);
            }
            Block::List if !nested_list => {
                self.break_line();
                let g = self.gap_size();
                self.add_gap(g);
            }
            Block::Cell => {
                if self.started {
                    self.x += CELL_GAP;
                    self.space = false;
                }
            }
            _ => self.break_line(),
        }
        self.stack.push(Ctx { block, left: self.left });
        self.pre_start = block == Block::Pre;
        match block {
            Block::Quote | Block::Indent | Block::List => {
                self.left = (self.left + INDENT).min(self.right - 40).max(self.left);
                self.x = self.left;
            }
            Block::Item(marker) => self.marker(marker),
            _ => {}
        }
    }

    fn close(&mut self) {
        let Some(ctx) = self.stack.pop() else { return };
        let nested_list = ctx.block == Block::List && self.stack.iter().any(|c| matches!(c.block, Block::List));
        match ctx.block {
            Block::Cell => {}
            _ => self.break_line(),
        }
        self.left = ctx.left;
        self.x = if self.cur.is_empty() { self.left } else { self.x };
        match ctx.block {
            Block::Para | Block::Heading(_) | Block::Pre | Block::Quote | Block::Table => {
                let g = self.gap_size();
                self.add_gap(g);
            }
            Block::List if !nested_list => {
                let g = self.gap_size();
                self.add_gap(g);
            }
            _ => {}
        }
    }

    /// リストの印を字下げの左に置く
    fn marker(&mut self, marker: Marker) {
        match marker {
            Marker::Bullet(level) => {
                let size = 5;
                let x = self.left - MARKER_GAP - size - 2;
                self.cur.push(Piece { x: x.max(0), w: size, h: size, kind: PieceKind::Bullet(level) });
            }
            Marker::Number(n) => {
                let text = alloc::format!("{n}.");
                let w = self.m.text_width(Font::Normal, &text);
                let x = (self.left - MARKER_GAP - w).max(0);
                self.cur.push(Piece { x, w, h: 0, kind: PieceKind::Text { text, font: Font::Normal, link: None } });
            }
        }
        self.x = self.left;
    }

    fn text(&mut self, s: &str, style: Style) {
        let font = font_of(style);
        if self.in_pre() {
            let s = if core::mem::take(&mut self.pre_start) { s.strip_prefix('\n').unwrap_or(s) } else { s };
            self.pre_text(s, font, style.link);
            return;
        }
        let mut word = String::new();
        for c in s.chars() {
            if c.is_ascii_whitespace() {
                if !word.is_empty() {
                    self.word(&word, font, style.link);
                    word.clear();
                }
                if self.started {
                    self.space = true;
                }
            } else {
                word.push(if c == NBSP { ' ' } else { c });
            }
        }
        if !word.is_empty() {
            self.word(&word, font, style.link);
        }
    }

    /// 単語を 1 つ置く（前にまとめた空白があれば空白をはさむ）
    fn word(&mut self, word: &str, font: Font, link: Option<u32>) {
        let mut sw = if self.space { self.m.text_width(font, " ") } else { 0 };
        let w = self.m.text_width(font, word);
        if self.started && self.x + sw + w > self.right {
            self.finish_line();
            sw = 0;
        }
        if w > self.right - self.x {
            // 1 行に入らない: 入るだけずつ切って置く
            let mut rest = word;
            if sw > 0 {
                self.x += sw;
            }
            while !rest.is_empty() {
                let n = self.fit(font, rest, self.right - self.x);
                let n = if n == 0 && !self.started { first_char_len(rest) } else { n };
                if n == 0 {
                    self.finish_line();
                    continue;
                }
                self.place(&rest[..n], font, link, false);
                rest = &rest[n..];
                if !rest.is_empty() {
                    self.finish_line();
                }
            }
            self.space = false;
            return;
        }
        self.place(word, font, link, sw > 0);
        self.space = false;
    }

    /// 文字を今の行に足す。同じ文字の種類・リンクの部品が直前にあればつなげる
    fn place(&mut self, s: &str, font: Font, link: Option<u32>, space_before: bool) {
        if let Some(Piece { x, w, kind: PieceKind::Text { text, font: f, link: l }, .. }) = self.cur.last_mut()
            && *f == font
            && *l == link
            && *x + *w == self.x
            && self.started
        {
            if space_before {
                text.push(' ');
            }
            text.push_str(s);
            *w = self.m.text_width(font, text);
            self.x = *x + *w;
            self.started = true;
            return;
        }
        if space_before {
            self.x += self.m.text_width(font, " ");
        }
        let w = self.m.text_width(font, s);
        self.cur.push(Piece { x: self.x, w, h: 0, kind: PieceKind::Text { text: String::from(s), font, link } });
        self.x += w;
        self.started = true;
    }

    /// 幅 `max` に入る `s` の頭のバイト数（文字の切れ目）
    fn fit(&self, font: Font, s: &str, max: i32) -> usize {
        if max <= 0 {
            return 0;
        }
        let mut end = 0;
        for (i, c) in s.char_indices() {
            let next = i + c.len_utf8();
            if self.m.text_width(font, &s[..next]) > max {
                break;
            }
            end = next;
        }
        end
    }

    fn pre_text(&mut self, s: &str, font: Font, link: Option<u32>) {
        let mut seg = String::new();
        for c in s.chars() {
            match c {
                '\n' => {
                    self.pre_segment(&seg, font, link);
                    seg.clear();
                    self.finish_line();
                }
                '\r' => {}
                '\t' => {
                    let n = 8 - (self.col + seg.chars().count()) % 8;
                    seg.extend(core::iter::repeat_n(' ', n));
                }
                NBSP => seg.push(' '),
                c => seg.push(c),
            }
        }
        self.pre_segment(&seg, font, link);
    }

    /// `Pre` の 1 行の中の文字を置く（幅を越えたら折り返す）
    fn pre_segment(&mut self, seg: &str, font: Font, link: Option<u32>) {
        let mut rest = seg;
        while !rest.is_empty() {
            let n = self.fit(font, rest, self.right - self.x);
            let n = if n == 0 && !self.started { first_char_len(rest) } else { n };
            if n == 0 {
                self.finish_line();
                continue;
            }
            self.col += rest[..n].chars().count();
            self.place(&rest[..n], font, link, false);
            rest = &rest[n..];
            if !rest.is_empty() {
                self.finish_line();
            }
        }
    }

    fn line_break(&mut self) {
        // 空の行でも 1 行分の高さを取る（<br><br> で空いた行ができる）
        self.finish_line();
    }

    fn rule(&mut self) {
        self.break_line();
        let g = self.gap_size() / 2;
        self.add_gap(g);
        let w = self.right - self.left;
        self.cur.push(Piece { x: self.left, w, h: 2, kind: PieceKind::Rule });
        self.finish_line();
        self.add_gap(g);
    }

    fn image(&mut self, index: u32, w: i32, h: i32, link: Option<u32>) {
        let avail = (self.right - self.left).max(1);
        let (w, h) =
            if w > avail { (avail, (h as i64 * avail as i64 / w.max(1) as i64).max(1) as i32) } else { (w, h) };
        let sw = if self.space { self.m.text_width(Font::Normal, " ") } else { 0 };
        if self.started && self.x + sw + w > self.right {
            self.finish_line();
        } else {
            self.x += sw;
        }
        self.cur.push(Piece { x: self.x, w, h, kind: PieceKind::Image { index, link } });
        self.x += w;
        self.started = true;
        self.space = false;
    }
}

fn first_char_len(s: &str) -> usize {
    s.chars().next().map_or(0, char::len_utf8)
}
