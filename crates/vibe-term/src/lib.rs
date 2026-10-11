//! 端末の制御文字と ANSI エスケープシーケンス（よく使う一部）の解釈
//!
//! 文字を 1 つずつ `Parser::feed` に渡すと、画面に対してすべき操作（`Action`）が返る。
//! 画面への反映は呼び出し側（コンソール）が行う。
//!
//! 対応しているもの:
//! - 制御文字: `\n` `\r` `\x08`（左へ 1 つ、消さない） `\t`
//! - `ESC [ n A/B/C/D`（上下左右に移動）, `ESC [ r ; c H`（位置指定、1 始まり）
//! - `ESC [ n K`（行の消去）, `ESC [ n J`（画面の消去）
//! - `ESC [ ... m`（色: 0 / 1 / 22 / 30–37 / 39 / 40–47 / 49 / 90–97 / 100–107）

#![cfg_attr(not(test), no_std)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Print(char),
    Newline,
    CarriageReturn,
    Backspace,
    Tab,
    CursorUp(u16),
    CursorDown(u16),
    CursorForward(u16),
    CursorBack(u16),
    /// 0 始まりの (行, 桁)
    CursorPosition(u16, u16),
    /// 0: カーソルから行末, 1: 行頭からカーソル, 2: 行全体
    EraseInLine(u16),
    /// 0: カーソルから画面の終わり, 1: 画面の始めからカーソル, 2: 画面全体
    EraseInDisplay(u16),
    /// 色などの属性（`m` の引数 1 つ分）
    Graphics(Sgr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sgr {
    Reset,
    Bold,
    Normal,
    /// ANSI の色番号 0〜7（黒 赤 緑 黄 青 紫 水 白）, 明るいか
    Foreground(u8, bool),
    DefaultForeground,
    Background(u8, bool),
    DefaultBackground,
}

const MAX_PARAMS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
}

pub struct Parser {
    state: State,
    params: [u16; MAX_PARAMS],
    count: usize,
    /// `m` の引数を 1 つずつ返すための残り
    pending: [u16; MAX_PARAMS],
    pending_len: usize,
    pending_pos: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub const fn new() -> Self {
        Parser {
            state: State::Ground,
            params: [0; MAX_PARAMS],
            count: 0,
            pending: [0; MAX_PARAMS],
            pending_len: 0,
            pending_pos: 0,
        }
    }

    /// 1 文字受け取る。`m` は引数の数だけ操作があるので、`next_pending` で残りを取り出すこと
    pub fn feed(&mut self, c: char) -> Option<Action> {
        match self.state {
            State::Ground => match c {
                '\x1b' => {
                    self.state = State::Escape;
                    None
                }
                '\n' => Some(Action::Newline),
                '\r' => Some(Action::CarriageReturn),
                '\x08' => Some(Action::Backspace),
                '\t' => Some(Action::Tab),
                c if (c as u32) < 0x20 || c == '\x7f' => None,
                c => Some(Action::Print(c)),
            },
            State::Escape => {
                if c == '[' {
                    self.state = State::Csi;
                    self.params = [0; MAX_PARAMS];
                    self.count = 0;
                } else {
                    self.state = State::Ground; // 対応していないエスケープは捨てる
                }
                None
            }
            State::Csi => match c {
                '0'..='9' => {
                    if self.count == 0 {
                        self.count = 1;
                    }
                    let p = &mut self.params[self.count - 1];
                    *p = p.saturating_mul(10).saturating_add(c as u16 - '0' as u16);
                    None
                }
                ';' => {
                    if self.count == 0 {
                        self.count = 1;
                    }
                    if self.count < MAX_PARAMS {
                        self.count += 1;
                    }
                    None
                }
                '\x40'..='\x7e' => {
                    self.state = State::Ground;
                    self.finish(c)
                }
                _ => None, // 中間バイトなどは無視
            },
        }
    }

    /// `m` で返し切れなかった残りの操作
    pub fn next_pending(&mut self) -> Option<Action> {
        while self.pending_pos < self.pending_len {
            let p = self.pending[self.pending_pos];
            self.pending_pos += 1;
            if let Some(sgr) = sgr(p) {
                return Some(Action::Graphics(sgr));
            }
        }
        None
    }

    fn param(&self, i: usize, default: u16) -> u16 {
        if i < self.count && self.params[i] != 0 { self.params[i] } else { default }
    }

    fn finish(&mut self, c: char) -> Option<Action> {
        let n = self.param(0, 1);
        Some(match c {
            'A' => Action::CursorUp(n),
            'B' => Action::CursorDown(n),
            'C' => Action::CursorForward(n),
            'D' => Action::CursorBack(n),
            'H' | 'f' => Action::CursorPosition(self.param(0, 1) - 1, self.param(1, 1) - 1),
            'K' => Action::EraseInLine(self.param(0, 0)),
            'J' => Action::EraseInDisplay(self.param(0, 0)),
            'm' => {
                let count = self.count.max(1); // 引数なしは 0（リセット）
                self.pending = self.params;
                self.pending_len = count;
                self.pending_pos = 0;
                return self.next_pending();
            }
            _ => return None,
        })
    }
}

fn sgr(p: u16) -> Option<Sgr> {
    Some(match p {
        0 => Sgr::Reset,
        1 => Sgr::Bold,
        22 => Sgr::Normal,
        30..=37 => Sgr::Foreground((p - 30) as u8, false),
        39 => Sgr::DefaultForeground,
        40..=47 => Sgr::Background((p - 40) as u8, false),
        49 => Sgr::DefaultBackground,
        90..=97 => Sgr::Foreground((p - 90) as u8, true),
        100..=107 => Sgr::Background((p - 100) as u8, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<Action> {
        let mut p = Parser::new();
        let mut out = Vec::new();
        for c in s.chars() {
            if let Some(a) = p.feed(c) {
                out.push(a);
                while let Some(a) = p.next_pending() {
                    out.push(a);
                }
            }
        }
        out
    }

    #[test]
    fn plain_text_and_controls() {
        assert_eq!(
            parse("a\r\n\x08\tb\x07"),
            [
                Action::Print('a'),
                Action::CarriageReturn,
                Action::Newline,
                Action::Backspace,
                Action::Tab,
                Action::Print('b')
            ]
        );
    }

    #[test]
    fn cursor_movement_with_defaults() {
        assert_eq!(parse("\x1b[D\x1b[3C\x1b[0A\x1b[12B"), [
            Action::CursorBack(1),
            Action::CursorForward(3),
            Action::CursorUp(1),
            Action::CursorDown(12)
        ]);
    }

    #[test]
    fn position_is_zero_based() {
        assert_eq!(parse("\x1b[H\x1b[5;10H\x1b[;3f"), [
            Action::CursorPosition(0, 0),
            Action::CursorPosition(4, 9),
            Action::CursorPosition(0, 2)
        ]);
    }

    #[test]
    fn erase() {
        assert_eq!(parse("\x1b[K\x1b[2K\x1b[2J"), [
            Action::EraseInLine(0),
            Action::EraseInLine(2),
            Action::EraseInDisplay(2)
        ]);
    }

    #[test]
    fn graphics_with_several_params() {
        assert_eq!(parse("\x1b[1;31;44mX\x1b[m"), [
            Action::Graphics(Sgr::Bold),
            Action::Graphics(Sgr::Foreground(1, false)),
            Action::Graphics(Sgr::Background(4, false)),
            Action::Print('X'),
            Action::Graphics(Sgr::Reset),
        ]);
        assert_eq!(parse("\x1b[92;0m"), [Action::Graphics(Sgr::Foreground(2, true)), Action::Graphics(Sgr::Reset)]);
    }

    #[test]
    fn unknown_sequences_are_dropped() {
        // ESC + 1 文字（2 文字のエスケープ）は両方捨てる。ESC [ ? 25 l のような未対応の CSI も捨てる
        assert_eq!(parse("\x1b]x\x1b[?25lA\x1b[5z"), [Action::Print('x'), Action::Print('A')]);
    }
}
