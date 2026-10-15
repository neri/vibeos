//! HTML の字句解析（HTML5 の仕様の tokenizer をごく簡単にしたもの）と文字参照
//!
//! - 開始タグ（名前は小文字、属性の名前も小文字、値は文字参照を戻したもの。同じ名前の 2 つ目からは捨てる）、
//!   終了タグ、文字（文字参照を戻したもの）を順に返す。コメント・`<!DOCTYPE>`・`<?…>` は飛ばす
//! - `script`・`style` などの中身は、閉じるタグまでをそのまま 1 つの文字として返す（`title`・`textarea` は
//!   文字参照だけ戻す）
//! - `<` の後ろが英字・`/`・`!`・`?` でなければ、ただの文字の `<`
//! - 入力が途中で切れていても止まらない（終わりの中途半端なタグは捨てる）

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Start { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    End { name: String },
    Text(String),
}

impl Token {
    /// 開始タグの属性の値
    pub fn attr(&self, name: &str) -> Option<&str> {
        match self {
            Token::Start { attrs, .. } => attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
}

/// 中身をそのまま読む要素（文字参照も戻さない）
const RAW_TEXT: &[&str] = &["script", "style", "xmp", "iframe", "noembed", "noframes"];
/// 中身を文字として読む要素（文字参照は戻す）
const RC_DATA: &[&str] = &["title", "textarea"];

pub struct Tokenizer<'a> {
    s: &'a str,
    pos: usize,
    /// 直前に開いた RAW_TEXT・RC_DATA の要素（中身を読む前）
    raw: Option<(String, bool)>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(s: &'a str) -> Tokenizer<'a> {
        Tokenizer { s, pos: 0, raw: None }
    }

    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    /// `</name` の位置（大文字小文字は区別しない。後ろが空白・`/`・`>`・終わり）
    fn find_end_tag(&self, name: &str) -> usize {
        let rest = self.rest().as_bytes();
        let n = name.len();
        let mut i = 0;
        while i + 2 + n <= rest.len() {
            if rest[i] == b'<'
                && rest[i + 1] == b'/'
                && rest[i + 2..i + 2 + n].eq_ignore_ascii_case(name.as_bytes())
                && rest.get(i + 2 + n).is_none_or(|&c| c.is_ascii_whitespace() || c == b'/' || c == b'>')
            {
                return i;
            }
            i += 1;
        }
        rest.len()
    }

    /// 文字（次のタグの手前まで）
    fn text(&mut self) -> Token {
        let bytes = self.rest().as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'<'
                && bytes.get(i + 1).is_some_and(|&c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
            {
                break;
            }
            i += 1;
        }
        let raw = &self.rest()[..i];
        self.pos += i;
        let mut out = String::with_capacity(raw.len());
        decode_entities(raw, false, &mut out);
        Token::Text(out)
    }

    /// `>` まで飛ばす（無ければ終わりまで）
    fn skip_to_gt(&mut self) {
        self.pos = match self.rest().find('>') {
            Some(i) => self.pos + i + 1,
            None => self.s.len(),
        };
    }

    /// `<` から始まるもの。返すものが無ければ None（コメントなど。続けて次を読む）
    fn markup(&mut self) -> Option<Token> {
        let rest = self.rest();
        if let Some(body) = rest.strip_prefix("<!--") {
            self.pos = match body.find("-->") {
                Some(i) => self.pos + 4 + i + 3,
                None => self.s.len(),
            };
            return None;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            self.skip_to_gt();
            return None;
        }
        if let Some(after) = rest.strip_prefix("</") {
            if !after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                self.skip_to_gt();
                return None;
            }
            let end = after.find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>').unwrap_or(after.len());
            let name = after[..end].to_ascii_lowercase();
            self.pos += 2 + end;
            if !self.rest().contains('>') {
                self.pos = self.s.len();
                return None;
            }
            self.skip_to_gt();
            return Some(Token::End { name });
        }
        self.start_tag()
    }

    fn start_tag(&mut self) -> Option<Token> {
        let b = self.s.as_bytes();
        let mut i = self.pos + 1;
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
            i += 1;
        }
        let name = self.s[start..i].to_ascii_lowercase();
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut self_closing = false;
        loop {
            while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
                self_closing = b[i] == b'/';
                i += 1;
            }
            if i >= b.len() {
                // 途中で切れたタグは捨てる
                self.pos = b.len();
                return None;
            }
            if b[i] == b'>' {
                i += 1;
                break;
            }
            self_closing = false;
            // 属性の名前（先頭の = も名前に入れる）
            let ns = i;
            i += 1;
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'/' | b'>' | b'=') {
                i += 1;
            }
            let aname = self.s[ns..i].to_ascii_lowercase();
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let mut value = String::new();
            if i < b.len() && b[i] == b'=' {
                i += 1;
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let (vs, ve);
                if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                    let q = b[i];
                    vs = i + 1;
                    i = vs;
                    while i < b.len() && b[i] != q {
                        i += 1;
                    }
                    ve = i;
                    i = (i + 1).min(b.len());
                } else {
                    vs = i;
                    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                        i += 1;
                    }
                    ve = i;
                }
                decode_entities(&self.s[vs..ve], true, &mut value);
            }
            if !attrs.iter().any(|(k, _)| *k == aname) {
                attrs.push((aname, value));
            }
        }
        self.pos = i;
        if !self_closing {
            if RAW_TEXT.contains(&name.as_str()) {
                self.raw = Some((name.clone(), false));
            } else if RC_DATA.contains(&name.as_str()) {
                self.raw = Some((name.clone(), true));
            }
        }
        Some(Token::Start { name, attrs, self_closing })
    }
}

impl Iterator for Tokenizer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        if let Some((name, rc)) = self.raw.take() {
            let end = self.find_end_tag(&name);
            let raw = &self.rest()[..end];
            self.pos += end;
            if !raw.is_empty() {
                let mut out = String::new();
                if rc {
                    decode_entities(raw, false, &mut out);
                } else {
                    out.push_str(raw);
                }
                return Some(Token::Text(out));
            }
        }
        while self.pos < self.s.len() {
            if self.rest().starts_with('<') && self.rest().len() > 1 {
                let c = self.rest().as_bytes()[1];
                if c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?') {
                    match self.markup() {
                        Some(t) => return Some(t),
                        None => continue,
                    }
                }
            }
            match self.text() {
                Token::Text(t) if t.is_empty() => {
                    // `<` だけが残った
                    self.pos += 1;
                    return Some(Token::Text(String::from("<")));
                }
                t => return Some(t),
            }
        }
        None
    }
}

/// ISO-8859-1 の 0xA0〜0xFF の文字参照の名前（`;` が無くても読む古い書き方の対象）
const LATIN1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not", "shy",
    "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1", "ordm",
    "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig",
    "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve",
    "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN",
    "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave", "eacute", "ecirc",
    "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml",
    "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
];

/// そのほかの文字参照（`;` が要る。amp・lt・gt・quot は `;` が無くても読む）
const NAMED: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("AMP", '&'),
    ("LT", '<'),
    ("GT", '>'),
    ("QUOT", '"'),
    ("COPY", '©'),
    ("REG", '®'),
    ("trade", '™'),
    ("hellip", '…'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("sbquo", '‚'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("bdquo", '„'),
    ("lsaquo", '‹'),
    ("rsaquo", '›'),
    ("bull", '•'),
    ("euro", '€'),
    ("dagger", '†'),
    ("Dagger", '‡'),
    ("permil", '‰'),
    ("prime", '′'),
    ("Prime", '″'),
    ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'),
    ("thinsp", '\u{2009}'),
    ("zwnj", '\u{200C}'),
    ("zwj", '\u{200D}'),
    ("lrm", '\u{200E}'),
    ("rlm", '\u{200F}'),
    ("larr", '←'),
    ("rarr", '→'),
    ("uarr", '↑'),
    ("darr", '↓'),
    ("harr", '↔'),
    ("lArr", '⇐'),
    ("rArr", '⇒'),
    ("le", '≤'),
    ("ge", '≥'),
    ("ne", '≠'),
    ("minus", '−'),
    ("hearts", '♥'),
];

fn lookup(name: &str) -> Option<char> {
    if let Some(i) = LATIN1.iter().position(|&n| n == name) {
        return char::from_u32(0xA0 + i as u32);
    }
    NAMED.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}

/// `;` が無くても読む名前か
fn legacy(name: &str) -> bool {
    matches!(name, "amp" | "lt" | "gt" | "quot" | "AMP" | "LT" | "GT" | "QUOT" | "COPY" | "REG")
        || LATIN1.contains(&name)
}

/// 文字参照（`&amp;`・`&#169;`・`&#xA9;`）を戻して `out` に足す。`in_attr` なら、`;` の無い古い書き方の
/// 後ろに英数字か `=` が続くものは戻さない（`?a=1&copy=2` のような URL を壊さないため）
pub fn decode_entities(s: &str, in_attr: bool, out: &mut String) {
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let (consumed, c) = entity(rest, in_attr);
        match c {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..consumed]),
        }
        rest = &rest[consumed..];
    }
    out.push_str(rest);
}

/// `&` から始まる文字参照を 1 つ読む。（読んだ長さ, 文字）。文字参照でなければ (1, None)
fn entity(s: &str, in_attr: bool) -> (usize, Option<char>) {
    let b = s.as_bytes();
    if b.get(1) == Some(&b'#') {
        let (hex, start) = if matches!(b.get(2), Some(b'x' | b'X')) { (true, 3) } else { (false, 2) };
        let mut i = start;
        let mut v: u32 = 0;
        while i < b.len() && (if hex { b[i].is_ascii_hexdigit() } else { b[i].is_ascii_digit() }) {
            let d = (b[i] as char).to_digit(16).unwrap_or(0);
            v = v.saturating_mul(if hex { 16 } else { 10 }).saturating_add(d);
            i += 1;
        }
        if i == start {
            return (1, None);
        }
        if b.get(i) == Some(&b';') {
            i += 1;
        }
        let c = match v {
            0x80..=0x9F => crate::charset::decode(&[v as u8], crate::charset::Charset::Windows1252).chars().next(),
            0 => None,
            _ => char::from_u32(v),
        };
        return (i, Some(c.unwrap_or('\u{FFFD}')));
    }
    let mut i = 1;
    while i < b.len() && i < 33 && b[i].is_ascii_alphanumeric() {
        i += 1;
    }
    let name = &s[1..i];
    if b.get(i) == Some(&b';')
        && let Some(c) = lookup(name)
    {
        return (i + 1, Some(c));
    }
    // `;` の無い古い書き方: 名前の頭で一番長いもの
    for end in (2..=name.len()).rev() {
        let n = &name[..end];
        if legacy(n) {
            let next = b.get(1 + end).copied();
            if in_attr && next.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'=') {
                return (1, None);
            }
            return (1 + end, lookup(n));
        }
    }
    (1, None)
}
