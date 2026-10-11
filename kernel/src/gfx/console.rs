//! テキストコンソール
//!
//! 画面の一番上の行（ステータスバー）を除いた範囲を文字の格子として使う。
//! 制御文字と ANSI エスケープシーケンスの一部を解釈する（vibe-term）。
//!
//! カーソルの左右移動（`\x08`, `ESC [ n D` / `C`）は行をまたいで前後の行へ回り込む。
//! 行をまたいだ入力行の編集（シェル）を相対移動だけで書けるようにするため。

use core::fmt::{self, Write};

use vibe_font::{FONT_8X8, Font};
use vibe_gfx::palette;
use vibe_term::{Action, Parser, Sgr};

use crate::drivers::vga::{self, SCREEN, Screen};
use crate::sync::IrqMutex;

const TAB_WIDTH: usize = 4;
/// コンソールの上に空けておく行数（ステータスバー）
pub const RESERVED_TOP_ROWS: usize = 1;

/// ANSI の色番号（黒 赤 緑 黄 青 紫 水 白）→ CGA パレット
const ANSI_TO_CGA: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
const DEFAULT_FG: u8 = palette::LIGHT_GRAY;
const DEFAULT_BG: u8 = palette::BLACK;

pub struct Console {
    font: &'static Font,
    /// 画面上の上端（ピクセル）
    top: usize,
    cols: usize,
    rows: usize,
    /// 0..=cols。cols は「行末まで書いた（次の文字で改行する）」状態
    col: usize,
    row: usize,
    fg: u8,
    bg: u8,
    bold: bool,
    parser: Parser,
    cursor_drawn: bool,
}

/// コンソール。SCREEN と両方取るときはこちらを先に取ること
static CONSOLE: IrqMutex<Console> = IrqMutex::new(Console {
    font: &FONT_8X8,
    top: 0,
    cols: 0,
    rows: 0,
    col: 0,
    row: 0,
    fg: DEFAULT_FG,
    bg: DEFAULT_BG,
    bold: false,
    parser: Parser::new(),
    cursor_drawn: false,
});

pub fn init() {
    let mut c = CONSOLE.lock();
    c.top = RESERVED_TOP_ROWS * c.font.height;
    c.cols = vga::width() / c.font.width;
    c.rows = vga::height() / c.font.height - RESERVED_TOP_ROWS;
    c.col = 0;
    c.row = 0;
}

/// 文字色と背景色（パレット番号）を直接設定する
pub fn set_color(fg: u8, bg: u8) {
    let mut c = CONSOLE.lock();
    c.fg = fg;
    c.bg = bg;
}

pub fn write_fmt(args: fmt::Arguments) {
    let mut console = CONSOLE.lock();
    if console.cols == 0 {
        return; // まだ初期化されていない
    }
    let mut screen = SCREEN.lock();
    console.hide_cursor(&mut screen);
    let _ = Writer { console: &mut console, screen: &mut screen }.write_fmt(args);
    console.show_cursor(&mut screen);
    screen.present();
}

/// コンソールの大きさ（桁, 行）
#[allow(dead_code)]
pub fn size() -> (usize, usize) {
    let c = CONSOLE.lock();
    (c.cols, c.rows)
}

struct Writer<'a> {
    console: &'a mut Console,
    screen: &'a mut Screen,
}

impl Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            if let Some(a) = self.console.parser.feed(ch) {
                self.console.apply(self.screen, a);
                while let Some(a) = self.console.parser.next_pending() {
                    self.console.apply(self.screen, a);
                }
            }
        }
        Ok(())
    }
}

impl Console {
    fn cell_y(&self, row: usize) -> usize {
        self.top + row * self.font.height
    }

    fn apply(&mut self, screen: &mut Screen, a: Action) {
        match a {
            Action::Print(ch) => {
                if self.col >= self.cols {
                    self.col = 0;
                    self.next_line(screen);
                }
                self.draw(screen, self.col, self.row, ch);
                self.col += 1;
            }
            Action::Newline => {
                self.col = 0;
                self.next_line(screen);
            }
            Action::CarriageReturn => self.col = 0,
            Action::Backspace => self.back(1),
            Action::Tab => {
                let next = (self.col / TAB_WIDTH + 1) * TAB_WIDTH;
                while self.col < next.min(self.cols) {
                    self.apply(screen, Action::Print(' '));
                }
            }
            Action::CursorUp(n) => self.row = self.row.saturating_sub(n as usize),
            Action::CursorDown(n) => self.row = (self.row + n as usize).min(self.rows - 1),
            Action::CursorForward(n) => self.forward(n as usize),
            Action::CursorBack(n) => self.back(n as usize),
            Action::CursorPosition(r, c) => {
                self.row = (r as usize).min(self.rows - 1);
                self.col = (c as usize).min(self.cols - 1);
            }
            Action::EraseInLine(mode) => {
                let col = self.col.min(self.cols);
                let (from, to) = match mode {
                    0 => (col, self.cols),
                    1 => (0, (col + 1).min(self.cols)),
                    _ => (0, self.cols),
                };
                self.erase_cells(screen, self.row, from, to);
            }
            Action::EraseInDisplay(mode) => {
                let col = self.col.min(self.cols);
                let (row, cols) = (self.row, self.cols);
                match mode {
                    0 => {
                        self.erase_cells(screen, row, col, cols);
                        self.erase_rows(screen, row + 1, self.rows);
                    }
                    1 => {
                        self.erase_rows(screen, 0, row);
                        self.erase_cells(screen, row, 0, (col + 1).min(cols));
                    }
                    _ => self.erase_rows(screen, 0, self.rows),
                }
            }
            Action::Graphics(sgr) => self.graphics(sgr),
        }
    }

    fn graphics(&mut self, sgr: Sgr) {
        match sgr {
            Sgr::Reset => {
                self.fg = DEFAULT_FG;
                self.bg = DEFAULT_BG;
                self.bold = false;
            }
            Sgr::Bold => {
                self.bold = true;
                if self.fg < 8 {
                    self.fg += 8;
                }
            }
            Sgr::Normal => {
                self.bold = false;
                if (8..16).contains(&self.fg) {
                    self.fg -= 8;
                }
            }
            Sgr::Foreground(c, bright) => {
                self.fg = ANSI_TO_CGA[c as usize & 7] + if bright || self.bold { 8 } else { 0 };
            }
            Sgr::DefaultForeground => self.fg = DEFAULT_FG,
            Sgr::Background(c, bright) => self.bg = ANSI_TO_CGA[c as usize & 7] + if bright { 8 } else { 0 },
            Sgr::DefaultBackground => self.bg = DEFAULT_BG,
        }
    }

    /// 左へ n 文字（行頭なら前の行の末尾へ）
    fn back(&mut self, n: usize) {
        for _ in 0..n {
            if self.col > 0 {
                self.col = self.col.min(self.cols) - 1;
            } else if self.row > 0 {
                self.row -= 1;
                self.col = self.cols - 1;
            }
        }
    }

    /// 右へ n 文字（行末なら次の行の先頭へ）
    fn forward(&mut self, n: usize) {
        for _ in 0..n {
            if self.col + 1 < self.cols {
                self.col += 1;
            } else if self.row + 1 < self.rows {
                self.row += 1;
                self.col = 0;
            } else {
                self.col = self.cols;
            }
        }
    }

    fn draw(&mut self, screen: &mut Screen, col: usize, row: usize, ch: char) {
        let (x, y) = (col * self.font.width, self.cell_y(row));
        screen.surface().draw_char(self.font, x as i32, y as i32, ch, self.fg, Some(self.bg));
        screen.mark_dirty(y, y + self.font.height);
    }

    fn erase_cells(&mut self, screen: &mut Screen, row: usize, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (w, h) = (self.font.width, self.font.height);
        let y = self.cell_y(row);
        screen.surface().fill_rect((from * w) as i32, y as i32, ((to - from) * w) as i32, h as i32, self.bg);
        screen.mark_dirty(y, y + h);
    }

    fn erase_rows(&mut self, screen: &mut Screen, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (y0, y1) = (self.cell_y(from), self.cell_y(to));
        screen.surface().fill_rect(0, y0 as i32, vga::width() as i32, (y1 - y0) as i32, self.bg);
        screen.mark_dirty(y0, y1);
    }

    fn next_line(&mut self, screen: &mut Screen) {
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        let h = self.font.height as i32;
        let (y0, y1) = (self.cell_y(0), self.cell_y(self.rows));
        screen.surface().scroll_up(0, y0 as i32, vga::width() as i32, (y1 - y0) as i32, h, self.bg);
        screen.mark_dirty(y0, y1);
    }

    /// カーソル（文字の下の 1 ピクセルの線）
    fn cursor_line(&self) -> (i32, i32, i32) {
        let col = self.col.min(self.cols - 1);
        let x = (col * self.font.width) as i32;
        let y = (self.cell_y(self.row) + self.font.height - 1) as i32;
        (x, y, self.font.width as i32)
    }

    fn show_cursor(&mut self, screen: &mut Screen) {
        let (x, y, w) = self.cursor_line();
        screen.surface().hline(x, y, w, self.fg);
        screen.mark_dirty(y as usize, y as usize + 1);
        self.cursor_drawn = true;
    }

    fn hide_cursor(&mut self, screen: &mut Screen) {
        if !self.cursor_drawn {
            return;
        }
        let (x, y, w) = self.cursor_line();
        screen.surface().hline(x, y, w, self.bg);
        screen.mark_dirty(y as usize, y as usize + 1);
        self.cursor_drawn = false;
    }
}
