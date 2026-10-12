//! スクリプトの構文木と、その組み立て
//!
//! ```text
//! list     := { and_or ( ; | & | 改行 ) }
//! and_or   := pipeline { ( && | || ) pipeline }
//! pipeline := command { | command }
//! command  := if | while | for | simple   （if・while・for の後ろにもリダイレクトを書ける）
//! if       := if list then list { elif list then list } [ else list ] fi
//! while    := while list do list done
//! for      := for NAME [ in 語... ] ( ; | 改行 ) do list done
//! simple   := { NAME=値 } { 語 | リダイレクト }
//! ```
//! 予約語（if then elif else fi while do done for in）は、コマンドの先頭の引用していない語のときだけ。

use alloc::string::String;
use alloc::vec::Vec;

use crate::ParseError;
use crate::lex::{Part, RedirOp, Tok, Token, Word, lex, plain};

pub type List = Vec<Item>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub and_or: AndOr,
    /// 後ろが `&`（裏で動かす）
    pub background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connector {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndOr {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redir {
    pub fd: u8,
    pub op: RedirOp,
    pub target: Word,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: Kind,
    pub redirects: Vec<Redir>,
    /// 元の文字列の中の範囲（パイプの中の if などを `sh -c` に渡すため）
    pub span: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Simple { assigns: Vec<(String, Word)>, words: Vec<Word> },
    If { arms: Vec<(List, List)>, otherwise: Option<List> },
    While { cond: List, body: List },
    /// items が None なら "$@"
    For { var: String, items: Option<Vec<Word>>, body: List },
}

const RESERVED: &[&str] = &["if", "then", "elif", "else", "fi", "while", "do", "done", "for", "in"];

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    src_len: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    /// 次がコマンドの先頭として予約語 `kw` か
    fn at_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(w)) if plain(w) == Some(kw))
    }

    fn at_any_keyword(&self, kws: &[&'static str]) -> bool {
        kws.iter().any(|k| self.at_keyword(k))
    }

    fn start(&self) -> usize {
        self.toks.get(self.pos).map_or(self.src_len, |t| t.start)
    }

    fn last_end(&self) -> usize {
        if self.pos == 0 { 0 } else { self.toks[self.pos - 1].end }
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(&Tok::Newline) {
            self.pos += 1;
        }
    }

    fn expect_keyword(&mut self, kw: &'static str) -> Result<(), ParseError> {
        self.skip_newlines();
        if self.at_keyword(kw) {
            self.pos += 1;
            return Ok(());
        }
        Err(match self.peek() {
            None => ParseError::Incomplete(kw),
            Some(t) => unexpected(t),
        })
    }

    /// `terms` のどれかの予約語（か終わり）まで
    fn list(&mut self, terms: &[&'static str]) -> Result<List, ParseError> {
        let mut items = Vec::new();
        loop {
            while matches!(self.peek(), Some(Tok::Newline | Tok::Semi)) {
                self.pos += 1;
            }
            match self.peek() {
                None if terms.is_empty() => return Ok(items),
                None => return Err(ParseError::Incomplete(terms[0])),
                _ if self.at_any_keyword(terms) => return Ok(items),
                _ => {}
            }
            let and_or = self.and_or()?;
            let background = match self.peek() {
                Some(Tok::Amp) => {
                    self.pos += 1;
                    true
                }
                Some(Tok::Semi | Tok::Newline) | None => false,
                _ if self.at_any_keyword(terms) => false,
                Some(t) => return Err(unexpected(t)),
            };
            items.push(Item { and_or, background });
        }
    }

    fn and_or(&mut self) -> Result<AndOr, ParseError> {
        let first = self.pipeline()?;
        let mut rest = Vec::new();
        loop {
            let c = match self.peek() {
                Some(Tok::AndIf) => Connector::And,
                Some(Tok::OrIf) => Connector::Or,
                _ => return Ok(AndOr { first, rest }),
            };
            self.pos += 1;
            self.skip_newlines();
            if self.peek().is_none() {
                return Err(ParseError::Incomplete(if c == Connector::And { "&&" } else { "||" }));
            }
            rest.push((c, self.pipeline()?));
        }
    }

    fn pipeline(&mut self) -> Result<Pipeline, ParseError> {
        let mut commands = alloc::vec![self.command()?];
        while self.peek() == Some(&Tok::Pipe) {
            self.pos += 1;
            self.skip_newlines();
            if self.peek().is_none() {
                return Err(ParseError::Incomplete("|"));
            }
            commands.push(self.command()?);
        }
        Ok(Pipeline { commands })
    }

    fn redirect(&mut self, fd: u8, op: RedirOp) -> Result<Redir, ParseError> {
        if fd > 2 {
            return Err(ParseError::BadFd(fd));
        }
        self.pos += 1;
        let name = match op {
            RedirOp::Read => "<",
            RedirOp::Write => ">",
            RedirOp::Append => ">>",
            RedirOp::Dup => ">&",
        };
        let Some(Tok::Word(w)) = self.peek().cloned() else { return Err(ParseError::MissingTarget(name)) };
        self.pos += 1;
        if op == RedirOp::Dup {
            match plain(&w).map(str::parse::<u8>) {
                Some(Ok(0..=2)) => {}
                Some(Ok(n)) => return Err(ParseError::BadFd(n)),
                _ => return Err(ParseError::MissingTarget(name)),
            }
        }
        Ok(Redir { fd, op, target: w })
    }

    /// 複合コマンドの後ろのリダイレクト
    fn trailing_redirects(&mut self) -> Result<Vec<Redir>, ParseError> {
        let mut out = Vec::new();
        while let Some(&Tok::Redir(fd, op)) = self.peek() {
            out.push(self.redirect(fd, op)?);
        }
        Ok(out)
    }

    fn command(&mut self) -> Result<Command, ParseError> {
        let start = self.start();
        let kind = if self.at_keyword("if") {
            self.pos += 1;
            let mut arms = Vec::new();
            let mut otherwise = None;
            let cond = self.list(&["then"])?;
            self.expect_keyword("then")?;
            let body = self.list(&["elif", "else", "fi"])?;
            arms.push((cond, body));
            loop {
                if self.at_keyword("elif") {
                    self.pos += 1;
                    let cond = self.list(&["then"])?;
                    self.expect_keyword("then")?;
                    let body = self.list(&["elif", "else", "fi"])?;
                    arms.push((cond, body));
                } else if self.at_keyword("else") {
                    self.pos += 1;
                    otherwise = Some(self.list(&["fi"])?);
                } else {
                    self.expect_keyword("fi")?;
                    break;
                }
            }
            Kind::If { arms, otherwise }
        } else if self.at_keyword("while") {
            self.pos += 1;
            let cond = self.list(&["do"])?;
            self.expect_keyword("do")?;
            let body = self.list(&["done"])?;
            self.expect_keyword("done")?;
            Kind::While { cond, body }
        } else if self.at_keyword("for") {
            self.pos += 1;
            let var = match self.peek() {
                Some(Tok::Word(w)) => match plain(w) {
                    Some(n) if is_name(n) => String::from(n),
                    _ => return Err(ParseError::Unexpected(String::from("for: bad variable name"))),
                },
                None => return Err(ParseError::Incomplete("do")),
                Some(t) => return Err(unexpected(t)),
            };
            self.pos += 1;
            let items = if self.at_keyword("in") {
                self.pos += 1;
                let mut v = Vec::new();
                while let Some(Tok::Word(w)) = self.peek() {
                    v.push(w.clone());
                    self.pos += 1;
                }
                Some(v)
            } else {
                None
            };
            match self.peek() {
                Some(Tok::Semi | Tok::Newline) => self.pos += 1,
                _ => {}
            }
            self.expect_keyword("do")?;
            let body = self.list(&["done"])?;
            self.expect_keyword("done")?;
            Kind::For { var, items, body }
        } else {
            return self.simple(start);
        };
        let redirects = self.trailing_redirects()?;
        Ok(Command { kind, redirects, span: (start, self.last_end()) })
    }

    fn simple(&mut self, start: usize) -> Result<Command, ParseError> {
        let mut assigns = Vec::new();
        let mut words = Vec::new();
        let mut redirects = Vec::new();
        loop {
            match self.peek().cloned() {
                Some(Tok::Word(w)) => {
                    if words.is_empty() {
                        if let Some(a) = assignment(&w) {
                            assigns.push(a);
                            self.pos += 1;
                            continue;
                        }
                        if let Some(k) = plain(&w).filter(|k| RESERVED.contains(k)) {
                            return Err(ParseError::Unexpected(String::from(k)));
                        }
                    }
                    words.push(w);
                    self.pos += 1;
                }
                Some(Tok::Redir(fd, op)) => redirects.push(self.redirect(fd, op)?),
                _ => break,
            }
        }
        if words.is_empty() && assigns.is_empty() && redirects.is_empty() {
            return Err(match self.peek() {
                None => ParseError::EmptyCommand,
                Some(t) => unexpected(t),
            });
        }
        Ok(Command { kind: Kind::Simple { assigns, words }, redirects, span: (start, self.last_end()) })
    }
}

fn unexpected(t: &Tok) -> ParseError {
    let s = match t {
        Tok::Word(w) => plain(w).unwrap_or("word"),
        Tok::Newline => "newline",
        Tok::Semi => ";",
        Tok::Amp => "&",
        Tok::AndIf => "&&",
        Tok::OrIf => "||",
        Tok::Pipe => "|",
        Tok::Redir(..) => "redirection",
    };
    ParseError::Unexpected(String::from(s))
}

pub fn is_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `NAME=値`（NAME は引用していない文字）なら (NAME, 値の語)
fn assignment(w: &Word) -> Option<(String, Word)> {
    let Some(Part::Lit(first, false)) = w.first() else { return None };
    let (name, rest) = first.split_once('=')?;
    if !is_name(name) {
        return None;
    }
    let mut value: Word = Vec::new();
    if !rest.is_empty() {
        value.push(Part::Lit(String::from(rest), false));
    }
    value.extend(w[1..].iter().cloned());
    Some((String::from(name), value))
}

/// スクリプト全体を組み立てる。途中で終わっていれば `ParseError::Incomplete`（続きの行を読む）
pub fn parse_script(src: &str) -> Result<List, ParseError> {
    let toks = lex(src)?;
    let mut p = Parser { toks: &toks, pos: 0, src_len: src.len() };
    let list = p.list(&[])?;
    Ok(list)
}

/// テスト用: 単純なコマンドの語（展開しないで文字だけ）
#[cfg(test)]
pub fn words_of(c: &Command) -> Vec<String> {
    match &c.kind {
        Kind::Simple { words, .. } => words
            .iter()
            .map(|w| {
                w.iter()
                    .map(|p| match p {
                        Part::Lit(s, _) => s.clone(),
                        Part::Var(n, _) => alloc::format!("${n}"),
                        Part::Arith(e, _) => alloc::format!("$(({e}))"),
                        Part::Cmd(e, _) => alloc::format!("$({e})"),
                    })
                    .collect()
            })
            .collect(),
        _ => Vec::new(),
    }
}
