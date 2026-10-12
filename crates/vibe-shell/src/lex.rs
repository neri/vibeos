//! スクリプトの字句（語・演算子・改行）
//!
//! 語は「部分」の並び（`Part`）。引用符の中かどうかを部分ごとに覚えておき、展開のとき
//! （変数の値の分け方・ファイル名の展開）に使う。

use alloc::string::String;
use alloc::vec::Vec;

use crate::ParseError;

/// 語の部分。bool は引用符の中（か `\` で引用した）か
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Lit(String, bool),
    /// `$X`・`${X}`・`$?`・`$#`・`$@`・`$0`〜`$9`（名前だけを持つ）
    Var(String, bool),
    /// `$((式))` の中身
    Arith(String, bool),
    /// `$(コマンド)` の中身
    Cmd(String, bool),
}

pub type Word = Vec<Part>;

/// 語がただの文字（引用符も展開も無い）なら、その文字
pub fn plain(w: &Word) -> Option<&str> {
    match w.as_slice() {
        [Part::Lit(s, false)] => Some(s),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirOp {
    /// `<`
    Read,
    /// `>`
    Write,
    /// `>>`
    Append,
    /// `>&`（後ろの語は fd の番号）
    Dup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Word(Word),
    Newline,
    Semi,
    Amp,
    AndIf,
    OrIf,
    Pipe,
    Redir(u8, RedirOp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    /// 元の文字列の中の位置（バイト）
    pub start: usize,
    pub end: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 語の区切り（引用符の外で）
fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')')
}

struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(usize, char)>,
    i: usize,
}

impl Lexer<'_> {
    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).map(|c| c.1)
    }

    fn pos(&self) -> usize {
        self.chars.get(self.i).map_or(self.src.len(), |c| c.0)
    }

    /// `$` の後ろ（`$` は読んだ後）。展開でなければ `$` そのもの
    fn dollar(&mut self, quoted: bool, out: &mut Word) -> Result<(), ParseError> {
        match self.peek(0) {
            Some('(') if self.peek(1) == Some('(') => {
                self.i += 2;
                let body = self.until_close(2)?;
                out.push(Part::Arith(body, quoted));
            }
            Some('(') => {
                self.i += 1;
                let body = self.until_close(1)?;
                out.push(Part::Cmd(body, quoted));
            }
            Some('{') => {
                self.i += 1;
                let mut name = String::new();
                loop {
                    match self.peek(0) {
                        None => return Err(ParseError::Incomplete("}")),
                        Some('}') => {
                            self.i += 1;
                            break;
                        }
                        Some(c) => {
                            name.push(c);
                            self.i += 1;
                        }
                    }
                }
                let ok = (name.chars().next().is_some_and(is_name_start) && name.chars().all(is_name_char))
                    || matches!(name.as_str(), "?" | "#" | "@")
                    || (!name.is_empty() && name.chars().all(|c| c.is_ascii_digit()));
                if !ok {
                    return Err(ParseError::BadSubstitution);
                }
                out.push(Part::Var(name, quoted));
            }
            Some(c) if is_name_start(c) => {
                let mut name = String::new();
                while let Some(c) = self.peek(0).filter(|&c| is_name_char(c)) {
                    name.push(c);
                    self.i += 1;
                }
                out.push(Part::Var(name, quoted));
            }
            Some(c @ ('?' | '#' | '@' | '0'..='9')) => {
                self.i += 1;
                out.push(Part::Var(String::from(c), quoted));
            }
            _ => push_lit(out, "$", quoted),
        }
        Ok(())
    }

    /// `(` を `depth` 個読んだ後から、対応する `)` までの中身（`depth` 個の `)` で閉じる）
    fn until_close(&mut self, depth: usize) -> Result<String, ParseError> {
        let start = self.pos();
        let mut level = depth;
        let mut quote: Option<char> = None;
        loop {
            let Some(c) = self.peek(0) else { return Err(ParseError::Incomplete(")")) };
            self.i += 1;
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some('"'), '\\') => self.i += 1,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '\\') => self.i += 1,
                (None, '(') => level += 1,
                (None, ')') => {
                    level -= 1;
                    if level == 0 {
                        let end = self.chars[self.i - depth].0;
                        return Ok(String::from(&self.src[start..end]));
                    }
                }
                _ => {}
            }
        }
    }

    fn word(&mut self) -> Result<Word, ParseError> {
        let mut w = Word::new();
        while let Some(c) = self.peek(0) {
            if is_break(c) {
                break;
            }
            self.i += 1;
            match c {
                '\'' => {
                    let mut s = String::new();
                    loop {
                        match self.peek(0) {
                            None => return Err(ParseError::Incomplete("'")),
                            Some('\'') => {
                                self.i += 1;
                                break;
                            }
                            Some(c) => {
                                s.push(c);
                                self.i += 1;
                            }
                        }
                    }
                    // '' も空の語として残す
                    w.push(Part::Lit(s, true));
                }
                '"' => {
                    let mut any = false;
                    loop {
                        match self.peek(0) {
                            None => return Err(ParseError::Incomplete("\"")),
                            Some('"') => {
                                self.i += 1;
                                break;
                            }
                            Some('\\') if matches!(self.peek(1), Some('"' | '\\' | '$' | '`')) => {
                                let c = self.peek(1).unwrap();
                                push_lit(&mut w, c.encode_utf8(&mut [0; 4]), true);
                                self.i += 2;
                            }
                            Some('\\') if self.peek(1) == Some('\n') => self.i += 2,
                            Some('$') => {
                                self.i += 1;
                                self.dollar(true, &mut w)?;
                            }
                            Some(c) => {
                                push_lit(&mut w, c.encode_utf8(&mut [0; 4]), true);
                                self.i += 1;
                            }
                        }
                        any = true;
                    }
                    if !any {
                        w.push(Part::Lit(String::new(), true));
                    }
                }
                '\\' => match self.peek(0) {
                    None => return Err(ParseError::Incomplete("\\")),
                    // 行の続き
                    Some('\n') => self.i += 1,
                    Some(c) => {
                        push_lit(&mut w, c.encode_utf8(&mut [0; 4]), true);
                        self.i += 1;
                    }
                },
                '$' => self.dollar(false, &mut w)?,
                c => push_lit(&mut w, c.encode_utf8(&mut [0; 4]), false),
            }
        }
        Ok(w)
    }
}

/// 文字を足す（直前が同じ引用の文字ならつなげる）
fn push_lit(w: &mut Word, s: &str, quoted: bool) {
    if let Some(Part::Lit(last, q)) = w.last_mut() {
        if *q == quoted {
            last.push_str(s);
            return;
        }
    }
    w.push(Part::Lit(String::from(s), quoted));
}

pub fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut lx = Lexer { src, chars: src.char_indices().collect(), i: 0 };
    let mut out = Vec::new();
    while let Some(c) = lx.peek(0) {
        let start = lx.pos();
        let mut push = |lx: &mut Lexer, tok: Tok, n: usize| {
            lx.i += n;
            out.push(Token { tok, start, end: lx.pos() });
        };
        match c {
            '\n' => push(&mut lx, Tok::Newline, 1),
            c if c.is_whitespace() => lx.i += 1,
            '\\' if lx.peek(1) == Some('\n') => lx.i += 2,
            '#' => {
                while lx.peek(0).is_some_and(|c| c != '\n') {
                    lx.i += 1;
                }
            }
            ';' => push(&mut lx, Tok::Semi, 1),
            '&' if lx.peek(1) == Some('&') => push(&mut lx, Tok::AndIf, 2),
            '&' => push(&mut lx, Tok::Amp, 1),
            '|' if lx.peek(1) == Some('|') => push(&mut lx, Tok::OrIf, 2),
            '|' => push(&mut lx, Tok::Pipe, 1),
            '(' | ')' => return Err(ParseError::Unsupported(c)),
            _ => {
                // [n]< [n]> [n]>> [n]>&
                let (fd, at) = match (c.to_digit(10), lx.peek(1)) {
                    (Some(d), Some('<' | '>')) => (Some(d as u8), 1),
                    _ => (None, 0),
                };
                match lx.peek(at) {
                    Some('<') => push(&mut lx, Tok::Redir(fd.unwrap_or(0), RedirOp::Read), at + 1),
                    Some('>') => {
                        let fd = fd.unwrap_or(1);
                        match lx.peek(at + 1) {
                            Some('>') => push(&mut lx, Tok::Redir(fd, RedirOp::Append), at + 2),
                            Some('&') => push(&mut lx, Tok::Redir(fd, RedirOp::Dup), at + 2),
                            _ => push(&mut lx, Tok::Redir(fd, RedirOp::Write), at + 1),
                        }
                    }
                    _ => {
                        let w = lx.word()?;
                        out.push(Token { tok: Tok::Word(w), start, end: lx.pos() });
                    }
                }
            }
        }
    }
    Ok(out)
}
