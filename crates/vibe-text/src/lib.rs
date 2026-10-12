//! テキストのフィルタ（`grep`・`wc`・`head`・`tail`・`sort`）の中身
//!
//! 入出力（標準入力・ファイル）はアプリが行い、ここでは行の選び方・数え方・並べ方と
//! オプションの読み方だけを扱う（ホストの `cargo test` で確かめる）。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// オプション（`-x` を並べたもの、`-abc` のようにまとめても可）と、残りの引数。
/// 値を取るオプション（`-n 5` / `-n5`）は `with_value` に入れておく。`--` の後はすべて残りの引数。
/// `-5` のように数だけのものは `('#', "5")` として返す
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub flags: Vec<(char, Option<String>)>,
    pub rest: Vec<String>,
}

impl Options {
    pub fn has(&self, c: char) -> bool {
        self.flags.iter().any(|f| f.0 == c)
    }

    /// 最後に指定した値
    pub fn value(&self, c: char) -> Option<&str> {
        self.flags.iter().rev().find(|f| f.0 == c).and_then(|f| f.1.as_deref())
    }
}

/// 知らないオプション、値の無いオプション
#[derive(Debug, PartialEq, Eq)]
pub enum OptError {
    Unknown(char),
    MissingValue(char),
}

pub fn parse_options<'a>(
    args: impl IntoIterator<Item = &'a str>,
    known: &str,
    with_value: &str,
) -> Result<Options, OptError> {
    let mut o = Options::default();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--" {
            o.rest.extend(it.by_ref().map(String::from));
            break;
        }
        let Some(body) = a.strip_prefix('-').filter(|b| !b.is_empty()) else {
            o.rest.push(String::from(a));
            continue;
        };
        if body.bytes().all(|b| b.is_ascii_digit()) && known.contains('#') {
            o.flags.push(('#', Some(String::from(body))));
            continue;
        }
        for (i, c) in body.char_indices() {
            if with_value.contains(c) {
                let rest = &body[i + c.len_utf8()..];
                let v = if rest.is_empty() { it.next().ok_or(OptError::MissingValue(c))? } else { rest };
                o.flags.push((c, Some(String::from(v))));
                break;
            }
            if !known.contains(c) || c == '#' {
                return Err(OptError::Unknown(c));
            }
            o.flags.push((c, None));
        }
    }
    Ok(o)
}

/// 行に分ける（最後の改行の後ろが空なら行にしない。`\r\n` の `\r` は除く）
pub fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n').map(|l| {
        let l = l.strip_suffix('\n').unwrap_or(l);
        l.strip_suffix('\r').unwrap_or(l)
    })
}

/// grep の選び方
#[derive(Debug, Clone, Copy, Default)]
pub struct Grep<'a> {
    pub pattern: &'a str,
    /// -i: 大文字と小文字を区別しない（ASCII）
    pub ignore_case: bool,
    /// -v: 合わない行を選ぶ
    pub invert: bool,
}

impl Grep<'_> {
    pub fn matches(&self, line: &str) -> bool {
        let found = if self.ignore_case {
            contains_ignore_case(line, self.pattern)
        } else {
            line.contains(self.pattern)
        };
        found != self.invert
    }
}

fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// wc の数（行・語・バイト）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub lines: usize,
    pub words: usize,
    pub bytes: usize,
}

impl Counts {
    /// 続きのデータを足す。`in_word` は前のデータが語の途中で終わったか（続けて呼ぶときに渡す）
    pub fn add(&mut self, data: &[u8], in_word: &mut bool) {
        self.bytes += data.len();
        for &b in data {
            if b == b'\n' {
                self.lines += 1;
            }
            let space = b.is_ascii_whitespace();
            if !space && !*in_word {
                self.words += 1;
            }
            *in_word = !space;
        }
    }

    pub fn of(data: &[u8]) -> Counts {
        let mut c = Counts::default();
        c.add(data, &mut false);
        c
    }
}

/// head / tail の行数（`-n N` か `-N`。既定 10）
pub fn line_count(o: &Options) -> Result<usize, ()> {
    match o.value('n').or(o.value('#')) {
        None => Ok(10),
        Some(v) => v.parse().map_err(|_| ()),
    }
}

/// 先頭の n 行
pub fn head(text: &str, n: usize) -> Vec<&str> {
    lines(text).take(n).collect()
}

/// 末尾の n 行
pub fn tail(text: &str, n: usize) -> Vec<&str> {
    let all: Vec<&str> = lines(text).collect();
    all[all.len().saturating_sub(n)..].to_vec()
}

/// sort の並べ方
#[derive(Debug, Clone, Copy, Default)]
pub struct Sort {
    /// -r: 逆の順
    pub reverse: bool,
    /// -n: 先頭の数で比べる（数でない行は 0）
    pub numeric: bool,
    /// -u: 同じ行は 1 つにする
    pub unique: bool,
}

/// 行の先頭の整数（空白を飛ばし、符号を読み、続く数字まで）
fn leading_number(line: &str) -> i64 {
    let t = line.trim_start();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        v = v.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    if neg { -v } else { v }
}

/// 安定なマージソート（下から 2 つずつ）。標準の `sort_by` はアプリに入れると 30KB ほど大きくなるので
/// 小さく書いたもの
pub fn merge_sort<T: Copy>(v: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    let n = v.len();
    if n < 2 {
        return;
    }
    let mut buf: Vec<T> = v.to_vec();
    let mut width = 1;
    // src から dst へ幅 width の組を併合する。回ごとに v と buf を入れ替える
    let mut in_v = true;
    while width < n {
        let (src, dst): (&[T], &mut [T]) = if in_v { (&*v, &mut buf[..]) } else { (&buf[..], &mut *v) };
        let mut start = 0;
        while start < n {
            let mid = (start + width).min(n);
            let end = (start + 2 * width).min(n);
            let (mut i, mut j, mut k) = (start, mid, start);
            while i < mid && j < end {
                // 右が左より小さいときだけ右を取る（等しいものは左が先: 安定）
                if less(&src[j], &src[i]) {
                    dst[k] = src[j];
                    j += 1;
                } else {
                    dst[k] = src[i];
                    i += 1;
                }
                k += 1;
            }
            dst[k..k + mid - i].copy_from_slice(&src[i..mid]);
            k += mid - i;
            dst[k..k + end - j].copy_from_slice(&src[j..end]);
            start = end;
        }
        in_v = !in_v;
        width *= 2;
    }
    if !in_v {
        v.copy_from_slice(&buf);
    }
}

impl Sort {
    pub fn sort<'a>(&self, mut lines: Vec<&'a str>) -> Vec<&'a str> {
        if self.numeric {
            merge_sort(&mut lines, |a, b| (leading_number(a), *a) < (leading_number(b), *b));
        } else {
            merge_sort(&mut lines, |a, b| a < b);
        }
        if self.unique {
            lines.dedup();
        }
        if self.reverse {
            lines.reverse();
        }
        lines
    }
}

#[cfg(test)]
mod tests;
