//! 電卓の中身
//!
//! 数は固定小数点（小数点以下 6 桁、`SCALE` 倍した i64）で持つ。浮動小数点は使わない。
//! 計算は押した順に行う（`2 + 3 * 4 =` は 20）。表示できる大きさ（整数部 12 桁）を超えたり
//! 0 で割ったりすると "Error" になり、次のキーで消える。割り算と掛け算の端数は四捨五入する。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;

/// 1 を表す値（小数点以下 6 桁）
pub const SCALE: i64 = 1_000_000;
pub const FRACTION_DIGITS: usize = 6;
/// 入力できる整数部の桁数・表示できる大きさ
pub const MAX_DIGITS: usize = 12;
const LIMIT: i128 = 1_000_000_000_000 * SCALE as i128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Digit(u8),
    Point,
    Op(Op),
    Equals,
    /// 全部消す（C）
    Clear,
    /// 入力中の数だけ消す（CE）
    ClearEntry,
    /// 1 文字消す
    Back,
    /// 符号を変える
    Negate,
}

impl Key {
    /// キーボードの文字から（数字・. + - * / = と Enter の改行）
    pub fn from_char(c: char) -> Option<Key> {
        Some(match c {
            '0'..='9' => Key::Digit(c as u8 - b'0'),
            '.' | ',' => Key::Point,
            '+' => Key::Op(Op::Add),
            '-' => Key::Op(Op::Sub),
            '*' | 'x' | 'X' => Key::Op(Op::Mul),
            '/' => Key::Op(Op::Div),
            '=' | '\n' => Key::Equals,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub struct Calc {
    /// 左側の値（演算子を押したときの値・前の結果）
    acc: Option<i64>,
    pending: Option<Op>,
    /// 入力中の数（`typing` のとき）
    entry: String,
    typing: bool,
    /// 結果として表示している値（`typing` でないとき）
    shown: i64,
    error: bool,
}

impl Default for Calc {
    fn default() -> Self {
        Self::new()
    }
}

impl Calc {
    pub fn new() -> Calc {
        Calc { acc: None, pending: None, entry: String::new(), typing: false, shown: 0, error: false }
    }

    /// 表示する文字列
    pub fn display(&self) -> String {
        if self.error {
            String::from("Error")
        } else if self.typing {
            self.entry.clone()
        } else {
            format_fixed(self.shown)
        }
    }

    /// 今の値（入力中ならその数）
    fn current(&self) -> i64 {
        if self.typing { parse_fixed(&self.entry) } else { self.shown }
    }

    fn fail(&mut self) {
        *self = Calc::new();
        self.error = true;
    }

    pub fn press(&mut self, key: Key) {
        if self.error {
            *self = Calc::new();
            // 数の入力ならそのまま始める。それ以外は消すだけ
            if !matches!(key, Key::Digit(_) | Key::Point) {
                return;
            }
        }
        match key {
            Key::Digit(d) => {
                if !self.typing {
                    self.entry = String::from("0");
                    self.typing = true;
                }
                let (int_digits, frac_digits) = digit_counts(&self.entry);
                if self.entry.contains('.') {
                    if frac_digits < FRACTION_DIGITS {
                        self.entry.push((b'0' + d) as char);
                    }
                } else if self.entry == "0" || self.entry == "-0" {
                    self.entry.pop();
                    self.entry.push((b'0' + d) as char);
                } else if int_digits < MAX_DIGITS {
                    self.entry.push((b'0' + d) as char);
                }
            }
            Key::Point => {
                if !self.typing {
                    self.entry = String::from("0");
                    self.typing = true;
                }
                if !self.entry.contains('.') {
                    self.entry.push('.');
                }
            }
            Key::Op(op) => {
                if self.typing || self.pending.is_none() {
                    let cur = self.current();
                    let value = match (self.acc, self.pending) {
                        (Some(a), Some(p)) if self.typing => apply(a, p, cur),
                        _ => Some(cur),
                    };
                    let Some(v) = value else { return self.fail() };
                    self.acc = Some(v);
                    self.shown = v;
                }
                // 演算子を続けて押したときは置き換えるだけ
                self.pending = Some(op);
                self.typing = false;
            }
            Key::Equals => {
                let cur = self.current();
                let result = match (self.acc, self.pending) {
                    (Some(a), Some(p)) => apply(a, p, cur),
                    _ => Some(cur),
                };
                let Some(v) = result else { return self.fail() };
                self.acc = Some(v);
                self.shown = v;
                self.pending = None;
                self.typing = false;
            }
            Key::Clear => *self = Calc::new(),
            Key::ClearEntry => {
                self.entry = String::from("0");
                self.typing = true;
            }
            Key::Back => {
                if self.typing {
                    self.entry.pop();
                    if self.entry.is_empty() || self.entry == "-" {
                        self.entry = String::from("0");
                    }
                }
            }
            Key::Negate => {
                if self.typing {
                    if let Some(rest) = self.entry.strip_prefix('-') {
                        self.entry = String::from(rest);
                    } else if self.entry != "0" {
                        self.entry.insert(0, '-');
                    }
                } else {
                    self.shown = -self.shown;
                    if self.pending.is_none() {
                        self.acc = Some(self.shown);
                    }
                }
            }
        }
    }
}

/// (整数部の桁数, 小数部の桁数)
fn digit_counts(entry: &str) -> (usize, usize) {
    let digits = entry.trim_start_matches('-');
    match digits.split_once('.') {
        Some((i, f)) => (i.len(), f.len()),
        None => (digits.len(), 0),
    }
}

/// 割り算の四捨五入（0 から遠い方へ）
fn div_round(n: i128, d: i128) -> i128 {
    let q = n / d;
    let r = n % d;
    if 2 * r.abs() >= d.abs() { q + if (n < 0) == (d < 0) { 1 } else { -1 } } else { q }
}

/// 計算する。大きすぎるか 0 で割ったら None
pub fn apply(a: i64, op: Op, b: i64) -> Option<i64> {
    let (a, b) = (a as i128, b as i128);
    let r = match op {
        Op::Add => a + b,
        Op::Sub => a - b,
        Op::Mul => div_round(a * b, SCALE as i128),
        Op::Div => {
            if b == 0 {
                return None;
            }
            div_round(a * SCALE as i128, b)
        }
    };
    (r.abs() < LIMIT).then_some(r as i64)
}

/// "12.5" → 12_500_000（入力中の文字列なので形は正しい前提。おかしな文字は無視する）
pub fn parse_fixed(s: &str) -> i64 {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let mut v: i64 = 0;
    for c in int.chars().filter(char::is_ascii_digit) {
        v = v.saturating_mul(10).saturating_add((c as u8 - b'0') as i64);
    }
    v = v.saturating_mul(SCALE);
    let mut unit = SCALE / 10;
    for c in frac.chars().filter(char::is_ascii_digit).take(FRACTION_DIGITS) {
        v += (c as u8 - b'0') as i64 * unit;
        unit /= 10;
    }
    if neg { -v } else { v }
}

/// 12_500_000 → "12.5"（小数部の末尾の 0 は書かない）
pub fn format_fixed(v: i64) -> String {
    let mut s = String::new();
    if v < 0 {
        s.push('-');
    }
    let a = v.unsigned_abs();
    let (int, frac) = (a / SCALE as u64, a % SCALE as u64);
    s.push_str(&alloc::format!("{int}"));
    if frac != 0 {
        let f = alloc::format!("{frac:06}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(keys: &str) -> String {
        let mut c = Calc::new();
        for ch in keys.chars() {
            let k = match ch {
                'C' => Key::Clear,
                'E' => Key::ClearEntry,
                '<' => Key::Back,
                'n' => Key::Negate,
                c => Key::from_char(c).unwrap(),
            };
            c.press(k);
        }
        c.display()
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(run("12+34="), "46");
        assert_eq!(run("7/2="), "3.5");
        assert_eq!(run("5-8="), "-3");
        assert_eq!(run("6*7="), "42");
        assert_eq!(run("0.1+0.2="), "0.3");
        assert_eq!(run("1.5*1.5="), "2.25");
    }

    #[test]
    fn rounding() {
        assert_eq!(run("1/3="), "0.333333");
        assert_eq!(run("2/3="), "0.666667");
        assert_eq!(run("0.000001*0.5="), "0.000001");
        assert_eq!(run("0-2/3="), "-0.666667");
    }

    #[test]
    fn chained_operations_go_left_to_right() {
        assert_eq!(run("2+3*4="), "20");
        // 途中の結果が表示される
        assert_eq!(run("2+3*"), "5");
        // 演算子を続けて押すと置き換わる
        assert_eq!(run("5+-3="), "2");
        // 結果に続けて計算する
        assert_eq!(run("2+3=*10="), "50");
        // 結果の後に数を打つと新しい計算
        assert_eq!(run("2+3=7"), "7");
    }

    #[test]
    fn errors_clear_on_next_key() {
        assert_eq!(run("1/0="), "Error");
        assert_eq!(run("1/0=5"), "5");
        assert_eq!(run("1/0=+"), "0");
        assert_eq!(run("999999999999*10="), "Error");
        assert_eq!(run("999999999999+1="), "Error");
        assert_eq!(run("999999999998+1="), "999999999999");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(run("123<"), "12");
        assert_eq!(run("1<<"), "0");
        assert_eq!(run("12+34E5="), "17");
        assert_eq!(run("12+34C"), "0");
        assert_eq!(run("12n"), "-12");
        assert_eq!(run("12nn"), "12");
        assert_eq!(run("12n<<"), "0");
        assert_eq!(run("3+4=n"), "-7");
        assert_eq!(run("3+4=n+1="), "-6");
    }

    #[test]
    fn input_limits() {
        assert_eq!(run("0000"), "0");
        assert_eq!(run("1234567890123"), "123456789012");
        assert_eq!(run("1.2345678"), "1.234567");
        assert_eq!(run("1..2"), "1.2");
        assert_eq!(run(".5"), "0.5");
        assert_eq!(run("+3="), "3");
    }

    #[test]
    fn parse_and_format() {
        assert_eq!(parse_fixed("12.5"), 12_500_000);
        assert_eq!(parse_fixed("-0.000001"), -1);
        assert_eq!(format_fixed(-1), "-0.000001");
        assert_eq!(format_fixed(3 * SCALE), "3");
        assert_eq!(format_fixed(0), "0");
    }
}
