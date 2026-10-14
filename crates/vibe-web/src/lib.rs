//! 簡易ブラウザ `browse` の中身（docs/plans/browser.md）
//!
//! - `url`: URL（`http:` と `file:`）と相対 URL の解決
//! - `charset`: バイト列から文字へ（UTF-8・Windows-1252）と、フォントに無い文字の置き換え
//! - `html`: HTML の字句解析と文字参照
//! - `doc`: 字句の並びから、ブロックと文字の並び（`Document`）を作る。木は作らない
//! - `layout`: `Document` を決まった幅に並べて、行と行の中の部品にする
//! - `http`: HTTP の要求と、返事を届いた順に読む（見出し・Content-Length・chunked）
//!
//! どれも画面にもシステムコールにも触らないので、ホストの `cargo test` で確かめられる。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod charset;
pub mod doc;
pub mod html;
pub mod http;
pub mod layout;
pub mod url;

#[cfg(test)]
mod tests;
