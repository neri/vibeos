//! シェルの文法と実行
//!
//! - `parse`: 1 行を、コマンドの並びとつなぎ方に分ける（docs/plans/pipe.md §3.5。カーネルの予備のシェルが使う）
//! - `lex`・`ast`・`interp`・`arith`: `sh` のスクリプトの言語（docs/plans/user-shell.md §3.3）。
//!   `;`・`&&`・`||`・変数・`export`・引用・`if`・`while`・`for`・`test`・`$((…))`・`$(…)`・ファイル名の展開。
//!   プロセスの起動などは `interp::Host` に任せるので、ホストのテストでは偽物に差し替える
//!
//! `parse` の文法: `cmd [args] [< file] [> file | >> file] [2> file | 2>&1] { | cmd ... } [&]`
//!
//! - 空白で区切る。`"..."` と `'...'` の中は空白や記号もそのまま。`\` は次の 1 文字をそのまま
//!   （`"..."` の中では `\"` と `\\` だけ）
//! - リダイレクトは `[n]<`, `[n]>`, `[n]>>`, `[n]>&m`（n・m は 0〜2。n を省くと `<` は 0、ほかは 1）。
//!   書いた順に左から行う（`> F 2>&1` は両方 F へ、`2>&1 > F` は標準エラーだけ元の出力へ）
//! - `|` でつなぎ、最後の `&` で裏で動かす
//!
//! カーネルの中のシェルとユーザ空間のシェル（docs/plans/user-shell.md）の両方から使う。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod arith;
pub mod ast;
pub mod interp;
pub mod lex;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// リダイレクトの行き先
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `<` ファイルから読む
    Read(String),
    /// `>` ファイルに書く（作り直す）
    Write(String),
    /// `>>` ファイルに書き足す
    Append(String),
    /// `>&m` fd m と同じものにする
    Dup(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// つなぎ替える fd（0〜2）
    pub fd: u8,
    pub target: Target,
}

/// 1 つのコマンド
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Command {
    pub argv: Vec<String>,
    /// 書いた順
    pub redirects: Vec<Redirect>,
}

/// `|` でつないだコマンドの並び
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    /// 最後に `&` がある（裏で動かす）
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 途中で終わっている（対話のときは続きの行を読む）。足りないもの
    Incomplete(&'static str),
    /// 思っていないところにあった語・記号
    Unexpected(String),
    /// `${...}` の中が名前でない
    BadSubstitution,
    /// 使えない記号（`(` `)`）
    Unsupported(char),
    /// 引用符が閉じていない
    UnterminatedQuote(char),
    /// `\` で行が終わった
    TrailingBackslash,
    /// リダイレクトの後にファイル名が無い（記号）
    MissingTarget(&'static str),
    /// `|` の前後にコマンドが無い
    EmptyCommand,
    /// `&` が最後でない
    MisplacedAmpersand,
    /// 0〜2 以外の fd
    BadFd(u8),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseError::Incomplete(what) => write!(f, "unexpected end (missing {what})"),
            ParseError::Unexpected(what) => write!(f, "unexpected {what}"),
            ParseError::BadSubstitution => write!(f, "bad ${{...}}"),
            ParseError::Unsupported(c) => write!(f, "{c} is not supported"),
            ParseError::UnterminatedQuote(q) => write!(f, "missing closing {q}"),
            ParseError::TrailingBackslash => write!(f, "nothing after \\"),
            ParseError::MissingTarget(op) => write!(f, "missing file name after {op}"),
            ParseError::EmptyCommand => write!(f, "missing command around |"),
            ParseError::MisplacedAmpersand => write!(f, "& must be at the end"),
            ParseError::BadFd(n) => write!(f, "bad file descriptor {n}"),
        }
    }
}

/// 字句
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    Amp,
    /// fd, 種類（"<" ">" ">>" ">&"）
    Redir(u8, &'static str),
}

fn is_special(c: char) -> bool {
    matches!(c, '|' | '&' | '<' | '>')
}

fn lex(line: &str) -> Result<Vec<Token>, ParseError> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // [n]< [n]> [n]>> [n]>&
        let (fd, at) = match (c.to_digit(10), chars.get(i + 1)) {
            (Some(d), Some('<' | '>')) => (Some(d as u8), i + 1),
            _ => (None, i),
        };
        match chars[at] {
            '|' => {
                out.push(Token::Pipe);
                i += 1;
                continue;
            }
            '&' => {
                out.push(Token::Amp);
                i += 1;
                continue;
            }
            '<' => {
                out.push(Token::Redir(fd.unwrap_or(0), "<"));
                i = at + 1;
                continue;
            }
            '>' => {
                let fd = fd.unwrap_or(1);
                match chars.get(at + 1) {
                    Some('>') => {
                        out.push(Token::Redir(fd, ">>"));
                        i = at + 2;
                    }
                    Some('&') => {
                        out.push(Token::Redir(fd, ">&"));
                        i = at + 2;
                    }
                    _ => {
                        out.push(Token::Redir(fd, ">"));
                        i = at + 1;
                    }
                }
                continue;
            }
            _ => {}
        }
        // 語
        let mut word = String::new();
        while i < chars.len() && !chars[i].is_whitespace() && !is_special(chars[i]) {
            match chars[i] {
                q @ ('"' | '\'') => {
                    i += 1;
                    loop {
                        match chars.get(i) {
                            None => return Err(ParseError::UnterminatedQuote(q)),
                            Some(&c) if c == q => break,
                            Some('\\') if q == '"' && matches!(chars.get(i + 1), Some('"' | '\\')) => {
                                word.push(chars[i + 1]);
                                i += 2;
                            }
                            Some(&c) => {
                                word.push(c);
                                i += 1;
                            }
                        }
                    }
                    i += 1;
                }
                '\\' => {
                    let Some(&c) = chars.get(i + 1) else { return Err(ParseError::TrailingBackslash) };
                    word.push(c);
                    i += 2;
                }
                c => {
                    word.push(c);
                    i += 1;
                }
            }
        }
        out.push(Token::Word(word));
    }
    Ok(out)
}

/// 1 行を分ける。空の行（空白だけ）なら None
pub fn parse(line: &str) -> Result<Option<Pipeline>, ParseError> {
    let tokens = lex(line)?;
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut p = Pipeline::default();
    let mut cur = Command::default();
    let mut it = tokens.into_iter().peekable();
    while let Some(t) = it.next() {
        match t {
            Token::Word(w) => cur.argv.push(w),
            Token::Pipe => {
                if cur.argv.is_empty() {
                    return Err(ParseError::EmptyCommand);
                }
                p.commands.push(core::mem::take(&mut cur));
            }
            Token::Amp => {
                if it.peek().is_some() {
                    return Err(ParseError::MisplacedAmpersand);
                }
                p.background = true;
            }
            Token::Redir(fd, op) => {
                if fd > 2 {
                    return Err(ParseError::BadFd(fd));
                }
                let Some(Token::Word(w)) = it.next() else { return Err(ParseError::MissingTarget(op)) };
                let target = match op {
                    "<" => Target::Read(w),
                    ">" => Target::Write(w),
                    ">>" => Target::Append(w),
                    _ => match w.parse::<u8>() {
                        Ok(n @ 0..=2) => Target::Dup(n),
                        Ok(n) => return Err(ParseError::BadFd(n)),
                        Err(_) => return Err(ParseError::MissingTarget(">&")),
                    },
                };
                cur.redirects.push(Redirect { fd, target });
            }
        }
    }
    if cur.argv.is_empty() {
        // "a |" や、リダイレクトだけの行
        return Err(ParseError::EmptyCommand);
    }
    p.commands.push(cur);
    Ok(Some(p))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_script;
