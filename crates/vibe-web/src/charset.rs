//! バイト列から文字へ、と、フォントに無い文字の置き換え
//!
//! - 文字コードは UTF-8 と Windows-1252（ISO-8859-1・US-ASCII と書かれたものも、ブラウザと同じく
//!   Windows-1252 として読む）。ほかの名前（Shift_JIS など）は分からないので Windows-1252 で読む
//! - 決め方: HTTP の `Content-Type` の charset → 文書の先頭 1024 バイトの `<meta charset>` /
//!   `<meta http-equiv=Content-Type content="…; charset=…">` → UTF-8 として正しければ UTF-8 → Windows-1252
//! - フォント（`.VFN`）は ASCII だけなので、よく使う記号は ASCII に置き換え、ほかは `?` にする
//!   （DECISIONS.md D-068）。改行しない空白（U+00A0）はそのまま残す（レイアウトが空白として描く）

use alloc::string::String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Windows1252,
}

/// 文字コードの名前（`charset=` の値）から
pub fn from_label(label: &str) -> Charset {
    let l = label.trim().trim_matches(|c| c == '"' || c == '\'');
    if l.eq_ignore_ascii_case("utf-8") || l.eq_ignore_ascii_case("utf8") { Charset::Utf8 } else { Charset::Windows1252 }
}

/// `Content-Type` の値（`text/html; charset=utf-8`）から charset を取り出す
pub fn label_in_content_type(value: &str) -> Option<&str> {
    value.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("charset").then(|| v.trim().trim_matches(|c| c == '"' || c == '\''))
    })
}

/// 文書の文字コードを決める（`header` は HTTP の `Content-Type` の charset。無ければ None）
pub fn detect(bytes: &[u8], header: Option<&str>) -> Charset {
    if let Some(l) = header {
        return from_label(l);
    }
    if let Some(l) = meta_charset(&bytes[..bytes.len().min(1024)]) {
        return from_label(&l);
    }
    if core::str::from_utf8(bytes).is_ok() { Charset::Utf8 } else { Charset::Windows1252 }
}

/// 先頭の `<meta …>` から charset を探す（大文字小文字は区別しない）
fn meta_charset(head: &[u8]) -> Option<String> {
    let lower: String = head.iter().map(|&b| (b as char).to_ascii_lowercase()).collect();
    let mut rest = lower.as_str();
    while let Some(i) = rest.find("<meta") {
        let tag_end = rest[i..].find('>').map_or(rest.len(), |e| i + e);
        let tag = &rest[i..tag_end];
        if let Some(j) = tag.find("charset") {
            let v = tag[j + 7..].trim_start();
            if let Some(v) = v.strip_prefix('=') {
                let v = v.trim_start().trim_start_matches(['"', '\'']);
                let end = v.find(|c: char| c == '"' || c == '\'' || c == ';' || c == '/' || c.is_ascii_whitespace());
                let label = &v[..end.unwrap_or(v.len())];
                if !label.is_empty() {
                    return Some(String::from(label));
                }
            }
        }
        rest = &rest[tag_end..];
    }
    None
}

/// Windows-1252 の 0x80〜0x9F（ほかは ISO-8859-1 と同じ）
const CP1252_HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’',
    '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
];

/// バイト列を文字にする（UTF-8 の誤りは U+FFFD）
pub fn decode(bytes: &[u8], cs: Charset) -> String {
    match cs {
        Charset::Utf8 => {
            // 先頭の BOM は捨てる
            let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
            String::from_utf8_lossy(b).into_owned()
        }
        Charset::Windows1252 => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => CP1252_HIGH[(b - 0x80) as usize],
                _ => b as char,
            })
            .collect(),
    }
}

/// 改行しない空白
pub const NBSP: char = '\u{A0}';

/// 表示するときの文字（ASCII か NBSP）にして `out` に足す。消える文字（幅の無い空白など）は何も足さない
pub fn fold_into(c: char, out: &mut String) {
    if c.is_ascii() {
        out.push(c);
        return;
    }
    let s = match c {
        NBSP => {
            out.push(NBSP);
            return;
        }
        // 幅の無い文字・やわらかいハイフン・BOM
        '\u{AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' => return,
        // いろいろな空白
        '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => " ",
        '‘' | '’' | '‚' | '′' => "'",
        '“' | '”' | '„' | '″' => "\"",
        '‐' | '‑' | '‒' | '–' | '—' | '―' | '−' => "-",
        '…' => "...",
        '•' | '·' | '∙' | '●' => "*",
        '«' => "<<",
        '»' => ">>",
        '‹' => "<",
        '›' => ">",
        '©' => "(c)",
        '®' => "(R)",
        '™' => "(TM)",
        '×' => "x",
        '÷' => "/",
        '±' => "+/-",
        '°' => " deg",
        '€' => "EUR",
        '£' => "GBP",
        '¥' => "JPY",
        '¢' => "c",
        '§' => "S",
        '¶' => "P",
        '¦' => "|",
        '←' => "<-",
        '→' => "->",
        '↑' => "^",
        '↓' => "v",
        '⇐' => "<=",
        '⇒' => "=>",
        '≤' => "<=",
        '≥' => ">=",
        '≠' => "!=",
        '½' => "1/2",
        '¼' => "1/4",
        '¾' => "3/4",
        _ => "?",
    };
    out.push_str(s);
}

/// 文字列全体を `fold_into` で置き換えたもの
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        fold_into(c, &mut out);
    }
    out
}
