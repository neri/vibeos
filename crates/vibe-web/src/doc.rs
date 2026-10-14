//! 字句の並びから文書の形（`Document`）を作る
//!
//! 木は作らず、「ブロックを開く・閉じる」「文字（スタイル付き）」「改行」「水平線」「画像」「印（`id`）」の
//! 列にする。開いている要素の積み上げ（`stack`）だけを持ち、HTML5 の閉じ忘れの補いのうちよく出るものを
//! まねる:
//!
//! - ブロックが始まると、開いている `<p>` を閉じる
//! - `<li>` は前の `<li>` を、`<dt>`・`<dd>` は前の `<dt>`・`<dd>` を、`<tr>` は前の `<tr>` を、
//!   `<td>`・`<th>` は前のセルを閉じる（いちばん近いリスト・表の中だけを見る）
//! - 見出しの中で見出しが始まると前の見出しを閉じる。`<a>` の中の `<a>` も同じ
//! - 終了タグは、積み上げの中のいちばん近い同じ名前の要素までを閉じる。無ければ捨てる
//!
//! `script`・`style`・`select` などの中身は捨てる。`<title>` の中身はタイトルにする。
//! 文字は `charset::fold` で表示できる文字にしてから入れる。

use alloc::string::String;
use alloc::vec::Vec;

use crate::charset;
use crate::html::{Token, Tokenizer};

/// ブロックの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    /// 上下に間をあける段落（`p` など）
    Para,
    /// 改行するだけのまとまり（`div` など）
    Div,
    Heading(u8),
    /// 空白と改行をそのまま残す等幅の文字
    Pre,
    /// 字下げした引用（`blockquote`）
    Quote,
    /// 字下げだけ（`dd`）
    Indent,
    /// リスト（`ul`・`ol`）。中の `Item` を字下げする
    List,
    /// リストの項目と、その印
    Item(Marker),
    Table,
    Row,
    Cell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// 入れ子の深さ（0 から）で形を変える点
    Bullet(u8),
    Number(i32),
}

/// 文字のスタイル
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub mono: bool,
    /// 見出しの大きさ（1〜6。0 なら見出しではない）
    pub heading: u8,
    /// リンクなら `Document::links` の番号
    pub link: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Open(Block),
    Close,
    Text(String, Style),
    Break,
    Rule,
    /// `Document::images` の番号
    Image(u32),
    /// `id`・`<a name>` の印（`#…` の移動先）
    Anchor(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub src: String,
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub link: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub title: String,
    /// `<base href>`（相対 URL の元）
    pub base: Option<String>,
    pub items: Vec<Item>,
    /// リンクの `href`（解決していないもの）
    pub links: Vec<String>,
    pub images: Vec<Image>,
}

/// 開いている要素
struct Open {
    /// タグの名前
    name: String,
    /// ブロックなら閉じるときに `Item::Close` を出す
    block: Option<Block>,
    effect: Effect,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    Bold,
    Mono,
    Link(u32),
    /// 中身を捨てる
    Hide,
    /// 中身をタイトルにする
    Title,
}

/// 積み上げの深さの上限（これより深い要素は開かない）
const MAX_DEPTH: usize = 256;

/// 中身ごと捨てる要素
const HIDDEN: &[&str] = &[
    "script", "style", "template", "noembed", "noframes", "iframe", "object", "select", "textarea", "svg", "math",
    "datalist", "canvas", "audio", "video", "map",
];
/// 中を持たない要素
const VOID: &[&str] = &[
    "br", "hr", "img", "input", "meta", "link", "base", "wbr", "area", "col", "embed", "param", "source", "track",
    "keygen",
];
/// 開いている `<p>` を閉じるブロック
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "ul",
    "xmp",
    "dd",
    "dt",
    "plaintext",
    "caption",
    "legend",
];

fn block_of(name: &str) -> Option<Block> {
    Some(match name {
        "p" | "dl" | "figure" | "form" | "fieldset" => Block::Para,
        "div" | "address" | "article" | "aside" | "center" | "details" | "dialog" | "figcaption" | "footer"
        | "header" | "hgroup" | "main" | "nav" | "section" | "summary" | "dt" | "caption" | "legend" | "noscript"
        | "body" => Block::Div,
        "h1" => Block::Heading(1),
        "h2" => Block::Heading(2),
        "h3" => Block::Heading(3),
        "h4" => Block::Heading(4),
        "h5" => Block::Heading(5),
        "h6" => Block::Heading(6),
        "pre" | "xmp" | "listing" | "plaintext" => Block::Pre,
        "blockquote" => Block::Quote,
        "dd" => Block::Indent,
        "ul" | "ol" | "menu" | "dir" => Block::List,
        "table" => Block::Table,
        "tr" => Block::Row,
        "td" | "th" => Block::Cell,
        _ => return None,
    })
}

struct Builder {
    doc: Document,
    stack: Vec<Open>,
    /// 開いているリストごとの次の番号（`ol` なら Some）
    counters: Vec<Option<i32>>,
}

/// HTML から文書を作る
pub fn parse(html: &str) -> Document {
    let mut b = Builder { doc: Document::default(), stack: Vec::new(), counters: Vec::new() };
    for t in Tokenizer::new(html) {
        match t {
            Token::Text(s) => b.text(&s),
            Token::Start { ref name, self_closing, .. } => {
                let name = name.clone();
                b.start(&name, &t, self_closing);
            }
            Token::End { name } => b.end(&name),
        }
    }
    while !b.stack.is_empty() {
        b.pop();
    }
    b.doc.title = collapse(&b.doc.title);
    b.doc
}

/// ただの文字（text/plain）の文書: 全体を 1 つの `Pre` にする
pub fn plain(text: &str) -> Document {
    let mut doc = Document::default();
    doc.items.push(Item::Open(Block::Pre));
    doc.items.push(Item::Text(charset::fold(text), Style { mono: true, ..Style::default() }));
    doc.items.push(Item::Close);
    doc
}

/// 空白をまとめ、前後の空白を除く
fn collapse(s: &str) -> String {
    let mut out = String::new();
    for w in s.split_ascii_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

impl Builder {
    fn hidden(&self) -> bool {
        self.stack.iter().any(|o| o.effect == Effect::Hide)
    }

    fn style(&self) -> Style {
        let mut s = Style::default();
        for o in &self.stack {
            match o.effect {
                Effect::Bold => s.bold = true,
                Effect::Mono => s.mono = true,
                Effect::Link(i) => s.link = Some(i),
                _ => {}
            }
            match o.block {
                Some(Block::Heading(n)) => s.heading = n,
                Some(Block::Pre) => s.mono = true,
                _ => {}
            }
        }
        s
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let style = self.style();
        let folded = charset::fold(text);
        if let Some(Item::Text(prev, ps)) = self.doc.items.last_mut()
            && *ps == style
        {
            prev.push_str(&folded);
            return;
        }
        self.doc.items.push(Item::Text(folded, style));
    }

    fn text(&mut self, s: &str) {
        if self.stack.iter().any(|o| o.effect == Effect::Title) {
            self.doc.title.push_str(s);
            return;
        }
        if self.hidden() {
            return;
        }
        self.push_text(s);
    }

    /// 積み上げのいちばん上を閉じる
    fn pop(&mut self) {
        if let Some(o) = self.stack.pop() {
            if o.block.is_some() {
                self.doc.items.push(Item::Close);
            }
            if o.block == Some(Block::List) {
                self.counters.pop();
            }
        }
    }

    /// 上から見て `stop` のどれかより手前にある `names` のどれかの位置
    fn find_open(&self, names: &[&str], stop: &[&str]) -> Option<usize> {
        for (i, o) in self.stack.iter().enumerate().rev() {
            if names.contains(&o.name.as_str()) {
                return Some(i);
            }
            if stop.contains(&o.name.as_str()) {
                return None;
            }
        }
        None
    }

    fn pop_to(&mut self, i: usize) {
        while self.stack.len() > i {
            self.pop();
        }
    }

    fn start(&mut self, name: &str, t: &Token, self_closing: bool) {
        let hidden = self.hidden();
        if !hidden {
            self.close_implied(name);
        }
        if let Some(id) = t.attr("id").filter(|v| !v.is_empty())
            && !hidden
        {
            self.doc.items.push(Item::Anchor(String::from(id)));
        }
        match name {
            "html" | "head" | "tbody" | "thead" | "tfoot" | "colgroup" | "frameset" | "frame" => {
                return;
            }
            "title" => {
                if !self_closing {
                    self.open(name, None, Effect::Title);
                }
                return;
            }
            "base" => {
                if self.doc.base.is_none() {
                    self.doc.base = t.attr("href").map(String::from);
                }
                return;
            }
            _ => {}
        }
        if hidden {
            // 隠している中では、閉じるタグと合わせるために中を持つ要素だけ積む
            if !VOID.contains(&name) && !self_closing {
                self.open(name, None, Effect::Hide);
            }
            return;
        }
        if HIDDEN.contains(&name) {
            match name {
                "select" | "textarea" => self.push_text("[...]"),
                "svg" | "canvas" | "video" | "audio" | "object" | "iframe" => {
                    if let Some(alt) = t.attr("title").or(t.attr("aria-label")) {
                        let s = ["[", alt, "]"].concat();
                        self.push_text(&s);
                    }
                }
                _ => {}
            }
            if !self_closing {
                self.open(name, None, Effect::Hide);
            }
            return;
        }
        if VOID.contains(&name) {
            self.void(name, t);
            return;
        }
        if name == "a"
            && let Some(n) = t.attr("name").filter(|v| !v.is_empty())
        {
            self.doc.items.push(Item::Anchor(String::from(n)));
        }
        let effect = match name {
            "b" | "strong" | "th" => Effect::Bold,
            "code" | "kbd" | "samp" | "tt" => Effect::Mono,
            "a" => match t.attr("href") {
                Some(h) => {
                    self.doc.links.push(String::from(h));
                    Effect::Link(self.doc.links.len() as u32 - 1)
                }
                None => Effect::None,
            },
            _ => Effect::None,
        };
        let block = match name {
            "li" => {
                let level = self.stack.iter().filter(|o| o.block == Some(Block::List)).count();
                let marker = match self.counters.last_mut() {
                    Some(Some(n)) => {
                        if let Some(v) = t.attr("value").and_then(|v| v.trim().parse().ok()) {
                            *n = v;
                        }
                        let m = Marker::Number(*n);
                        *n += 1;
                        m
                    }
                    _ => Marker::Bullet(level.saturating_sub(1).min(255) as u8),
                };
                Some(Block::Item(marker))
            }
            _ => block_of(name),
        };
        if self_closing && block.is_none() {
            return;
        }
        if self.stack.len() >= MAX_DEPTH {
            return;
        }
        self.open(name, block, effect);
        if block == Some(Block::List) {
            let counter = (name == "ol").then(|| t.attr("start").and_then(|v| v.trim().parse().ok()).unwrap_or(1));
            self.counters.push(counter);
        }
        if self_closing {
            self.pop();
        }
    }

    fn open(&mut self, name: &str, block: Option<Block>, effect: Effect) {
        if let Some(b) = block {
            self.doc.items.push(Item::Open(b));
        }
        self.stack.push(Open { name: String::from(name), block, effect });
    }

    /// 開始タグ `name` が閉じさせる要素を閉じる
    fn close_implied(&mut self, name: &str) {
        const LISTS: &[&str] = &["ul", "ol", "menu", "dir"];
        const TABLE: &[&str] = &["table"];
        if CLOSES_P.contains(&name) {
            // いちばん近いブロックが <p> なら閉じる
            if let Some(i) = self.stack.iter().rposition(|o| o.block.is_some())
                && self.stack[i].name == "p"
            {
                self.pop_to(i);
            }
        }
        let found = match name {
            "li" => self.find_open(&["li"], LISTS),
            "dt" | "dd" => self.find_open(&["dt", "dd"], &["dl"]),
            "tr" => self.find_open(&["tr"], TABLE),
            "td" | "th" => self.find_open(&["td", "th"], &["tr", "table"]),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => match self.stack.last() {
                Some(o) if matches!(o.block, Some(Block::Heading(_))) => Some(self.stack.len() - 1),
                _ => None,
            },
            "a" => self.find_open(&["a"], &[]),
            _ => None,
        };
        if let Some(i) = found {
            self.pop_to(i);
        }
    }

    fn void(&mut self, name: &str, t: &Token) {
        match name {
            "br" => self.doc.items.push(Item::Break),
            "hr" => self.doc.items.push(Item::Rule),
            "img" => {
                let dim = |k: &str| t.attr(k).and_then(|v| v.trim().trim_end_matches("px").parse::<u32>().ok());
                let img = Image {
                    src: String::from(t.attr("src").unwrap_or("")),
                    alt: collapse(&charset::fold(t.attr("alt").unwrap_or(""))),
                    width: dim("width"),
                    height: dim("height"),
                    link: self.style().link,
                };
                self.doc.images.push(img);
                self.doc.items.push(Item::Image(self.doc.images.len() as u32 - 1));
            }
            "input" => {
                let kind = t.attr("type").unwrap_or("text").to_ascii_lowercase();
                let value = t.attr("value").filter(|v| !v.is_empty());
                let s = match kind.as_str() {
                    "hidden" => return,
                    "submit" | "button" | "reset" => {
                        ["[", value.unwrap_or(if kind == "reset" { "Reset" } else { "Submit" }), "]"].concat()
                    }
                    "checkbox" => String::from(if t.attr("checked").is_some() { "[x]" } else { "[ ]" }),
                    "radio" => String::from(if t.attr("checked").is_some() { "(*)" } else { "( )" }),
                    "image" => ["[", t.attr("alt").unwrap_or("Submit"), "]"].concat(),
                    _ => ["[", value.or(t.attr("placeholder")).unwrap_or("________"), "]"].concat(),
                };
                self.push_text(&s);
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        if name == "br" && !self.hidden() {
            self.doc.items.push(Item::Break);
            return;
        }
        if let Some(i) = self.stack.iter().rposition(|o| o.name == name) {
            self.pop_to(i);
        }
    }
}
