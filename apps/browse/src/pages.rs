//! アプリが作るページ（始めのページ・エラー・フォルダの一覧）。どれも HTML の文字列にして、ふつうのページと
//! 同じように読む

use alloc::format;
use alloc::string::String;

use vibe_rt::abi::DirEntry;

/// HTML の特別な文字を文字参照にする
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// 引数なしで起動したときのページ
pub fn start() -> String {
    String::from(
        "<title>Browse</title><h1>Browse</h1>\
         <p>A small web browser for VibeOS. It shows text, headings, lists and links. \
         It does not run JavaScript, does not use CSS, and cannot open HTTPS pages.</p>\
         <h2>Try</h2><ul>\
         <li><a href=\"file:///\">The floppy disk</a> (file:///)</li>\
         <li><a href=\"file:///DOCS/\">Documents</a> (file:///DOCS/)</li>\
         <li><a href=\"http://example.com/\">example.com</a> (needs a network)</li></ul>\
         <h2>Keys</h2><ul>\
         <li>Type a URL in the address field and press Enter. Ctrl+L goes to the address field.</li>\
         <li>Up, Down, Page Up, Page Down, Space, Home and End scroll the page.</li>\
         <li>Alt+Left and Alt+Right go back and forward. F5 loads the page again. Esc stops loading.</li></ul>",
    )
}

/// 開けなかったときのページ
pub fn error(url: &str, message: &str) -> String {
    format!(
        "<title>Cannot open the page</title><h1>Cannot open the page</h1><p>{}</p><p><code>{}</code></p>",
        escape(message),
        escape(url)
    )
}

/// フォルダの中身の一覧（フォルダが先、名前の順）
pub fn directory(path: &str, entries: &mut [DirEntry]) -> String {
    entries.sort_by(|a, b| (b.is_dir, a.name()).cmp(&(a.is_dir, b.name())));
    let shown = if path.ends_with('/') { String::from(path) } else { format!("{path}/") };
    let mut s = format!("<title>{0}</title><h1>{0}</h1><ul>", escape(&shown));
    if shown != "/" {
        s.push_str("<li><a href=\"../\">..</a> (parent folder)</li>");
    }
    for e in entries.iter() {
        let name = escape(e.name());
        if e.is_dir != 0 {
            s.push_str(&format!("<li><a href=\"{name}/\">{name}/</a></li>"));
        } else {
            s.push_str(&format!("<li><a href=\"{name}\">{name}</a> ({} bytes)</li>", e.size));
        }
    }
    s.push_str("</ul>");
    s
}
