//! ファイルマネージャの中身
//!
//! パスの組み立て（`join`・`parent`・`resolve`）、一覧の並べ方（フォルダが先、名前の順）と
//! 表示の形、拡張子とアプリの対応（`opener_for`）。画面にもシステムコールにも触らないので
//! ホストでテストできる。パスは "/" 区切りの絶対パス（ルートは "/"）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// ファイルの開き方
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opener {
    /// アプリにパスを渡して起動する（ウィンドウを開くアプリ）
    App(&'static str),
    /// 新しい端末でアプリにパスを渡して起動する（端末で動くアプリ）
    Terminal(&'static str),
    /// そのファイル自体を実行する
    Run,
}

/// 拡張子（大文字）→ 開き方
pub const ASSOCIATIONS: &[(&str, Opener)] = &[
    ("BMP", Opener::App("view")),
    ("TXT", Opener::Terminal("edit")),
    ("CFG", Opener::Terminal("edit")),
    ("INI", Opener::Terminal("edit")),
    ("LOG", Opener::Terminal("edit")),
    ("MD", Opener::Terminal("edit")),
    ("HTM", Opener::Terminal("edit")),
    ("ELF", Opener::Run),
    // スクリプト: 新しい端末で動かし、終わった後もその端末で打てるようにする（sh -i）
    ("SH", Opener::Terminal("sh -i")),
];

/// 拡張子（"." の後ろ。無ければ None）
pub fn extension(name: &str) -> Option<&str> {
    match name.rfind('.') {
        Some(0) | None => None,
        Some(i) => Some(&name[i + 1..]),
    }
}

/// ファイルの名前からの開き方。知らない拡張子なら None
pub fn opener_for(name: &str) -> Option<Opener> {
    let ext = extension(file_name(name))?;
    ASSOCIATIONS.iter().find(|(e, _)| e.eq_ignore_ascii_case(ext)).map(|&(_, o)| o)
}

/// パスの最後の名前
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// ディレクトリ `dir` の中の `name` のパス
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

/// 親のディレクトリ（ルートの親はルート）
pub fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => String::from(&path[..i]),
    }
}

/// `dir` から見た `input`（相対か絶対）を、"." と ".." を解いた絶対パスにする（名前は大文字に）
pub fn resolve(dir: &str, input: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let base = if input.starts_with('/') { "" } else { dir };
    for p in base.split('/').chain(input.split('/')) {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name.to_ascii_uppercase()),
        }
    }
    let mut out = String::new();
    for p in &parts {
        out.push('/');
        out.push_str(p);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// 一覧の 1 項目
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
}

/// フォルダを先に、それぞれ名前の順に並べる
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
}

/// 一覧に出す 1 行（名前とタブと、ファイルなら大きさ。部品の一覧が列に分けて描く）
pub fn list_line(e: &Entry) -> String {
    if e.is_dir { format!("{}\t", e.name) } else { format!("{}\t{}", e.name, e.size) }
}

/// 一覧に出すアイコンの名前（/SYSTEM/ICONS.VIA の中の名前）
pub fn icon_name(e: &Entry) -> &'static str {
    if e.is_dir {
        return "folder";
    }
    match e.name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("BMP") => "image",
        Some("SH") => "script",
        Some("ELF") => "app",
        _ => "file",
    }
}

/// 一覧の項目: ルートでなければ先頭に ".."（親へ）を置く
pub fn list_items(dir: &str, entries: &[Entry]) -> Vec<String> {
    let mut items = Vec::new();
    if dir != "/" {
        items.push(String::from(".."));
    }
    items.extend(entries.iter().map(list_line));
    items
}

/// 一覧で `index` 番目を選んだときの項目（".." なら None）
pub fn entry_at<'a>(dir: &str, entries: &'a [Entry], index: usize) -> Option<&'a Entry> {
    let offset = if dir != "/" { 1 } else { 0 };
    index.checked_sub(offset).and_then(|i| entries.get(i))
}

/// 一覧で名前が `name` の項目の位置（".." の分も数える）
pub fn index_of(dir: &str, entries: &[Entry], name: &str) -> Option<usize> {
    let offset = if dir != "/" { 1 } else { 0 };
    entries.iter().position(|e| e.name == name).map(|i| i + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool, size: u32) -> Entry {
        Entry { name: String::from(name), is_dir, size }
    }

    #[test]
    fn openers() {
        assert_eq!(opener_for("PLASMA.BMP"), Some(Opener::App("view")));
        assert_eq!(opener_for("/DOCS/guide.txt"), Some(Opener::Terminal("edit")));
        assert_eq!(opener_for("/BIN/CALC.ELF"), Some(Opener::Run));
        assert_eq!(opener_for("/DOCS/HI.SH"), Some(Opener::Terminal("sh -i")));
        assert_eq!(opener_for("DATA.BIN"), None);
        assert_eq!(opener_for("NOEXT"), None);
        assert_eq!(opener_for("/A.B/NOEXT"), None, "the dot is in the directory name");
        assert_eq!(extension(".HIDDEN"), None);
    }

    #[test]
    fn paths() {
        assert_eq!(join("/", "DOCS"), "/DOCS");
        assert_eq!(join("/DOCS", "A.TXT"), "/DOCS/A.TXT");
        assert_eq!(parent("/DOCS/A.TXT"), "/DOCS");
        assert_eq!(parent("/DOCS"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(file_name("/DOCS/A.TXT"), "A.TXT");
        assert_eq!(file_name("A.TXT"), "A.TXT");
    }

    #[test]
    fn resolve_paths() {
        assert_eq!(resolve("/DOCS", "copy.txt"), "/DOCS/COPY.TXT");
        assert_eq!(resolve("/DOCS", "/moved.txt"), "/MOVED.TXT");
        assert_eq!(resolve("/DOCS/SUB", "../x"), "/DOCS/X");
        assert_eq!(resolve("/", ".."), "/");
        assert_eq!(resolve("/A", "./b/../c"), "/A/C");
    }

    #[test]
    fn sorting_and_lines() {
        let mut v = vec![e("ZED.TXT", false, 5), e("DOCS", true, 0), e("ALPHA.BMP", false, 12345), e("BIN", true, 0)];
        sort_entries(&mut v);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["BIN", "DOCS", "ALPHA.BMP", "ZED.TXT"]);
        assert_eq!(list_line(&v[0]), "BIN\t");
        assert_eq!(list_line(&v[2]), "ALPHA.BMP\t12345");
        assert_eq!(icon_name(&v[0]), "folder");
        assert_eq!(icon_name(&v[2]), "image");
        assert_eq!(icon_name(&e("RUN.SH", false, 1)), "script");
        assert_eq!(icon_name(&e("SH.ELF", false, 1)), "app");
        assert_eq!(icon_name(&e("README.TXT", false, 1)), "file");
    }

    #[test]
    fn list_items_with_parent_entry() {
        let v = vec![e("A.TXT", false, 1), e("B.TXT", false, 2)];
        assert_eq!(list_items("/", &v).len(), 2);
        let items = list_items("/DOCS", &v);
        assert_eq!(items[0], "..");
        assert_eq!(entry_at("/DOCS", &v, 0), None);
        assert_eq!(entry_at("/DOCS", &v, 2).map(|e| e.name.as_str()), Some("B.TXT"));
        assert_eq!(entry_at("/", &v, 0).map(|e| e.name.as_str()), Some("A.TXT"));
        assert_eq!(index_of("/DOCS", &v, "B.TXT"), Some(2));
        assert_eq!(index_of("/", &v, "B.TXT"), Some(1));
        assert_eq!(index_of("/", &v, "C.TXT"), None);
    }
}
