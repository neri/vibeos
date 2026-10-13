//! テキストの端末
//!
//! 端末は端末のウィンドウの中身を文字の格子として使う。制御文字と ANSI エスケープシーケンスの
//! 一部を解釈する（vibe-term）。端末はいくつでも開ける（`open_terminal`）。それぞれにシェルが
//! 1 つ付き、タスクは「つながっている端末」（`sched::current_tty`）に書く。
//!
//! 最初の端末（0 番のウィンドウ）は起動直後から使うので静的な領域に置く。ほかの端末はヒープに置く。
//!
//! 文字: デスクトップの端末（32 ビットのウィンドウ）は固定幅のなめらかなフォント（gfx::fonts::mono、
//! 1 文字 8x16）、全画面（8 ビット）とフォントが無いときは 8x8 のビットマップのフォント。
//!
//! カーソルの左右移動（`\x08`, `ESC [ n D` / `C`）は行をまたいで前後の行へ回り込む。
//! 行をまたいだ入力行の編集（シェル）を相対移動だけで書けるようにするため。
//!
//! 端末は描いた文字を格子（`grid`）にも覚えておき、マウスでなぞった範囲を色を反転して見せ、
//! 離したときにその文字をクリップボードに入れる（`mouse`。デスクトップで gui から呼ばれる）。
//! 格子はヒープを使うので、スケジューラが動いてから（起動の途中の文字は覚えない）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write};

use vibe_font::{FONT_8X8, Font, VFont};
use vibe_gfx::palette;
use vibe_term::{Action, Parser, Sgr};

use crate::gui::{self, SCREEN, Screen, WinId, WinPixels};
use crate::sync::IrqMutex;
use crate::task::sched;

const TAB_WIDTH: usize = 4;
/// Alt+F1〜F4 で選べる端末の数（デスクトップでは `term` でいくつでも開ける）
const MAX_NUMBERED: usize = 4;

/// ANSI の色番号（黒 赤 緑 黄 青 紫 水 白）→ CGA パレット
const ANSI_TO_CGA: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
const DEFAULT_FG: u8 = palette::LIGHT_GRAY;
const DEFAULT_BG: u8 = palette::BLACK;

/// 端末の文字
#[derive(Clone, Copy)]
enum TermFont {
    Bitmap(&'static Font),
    /// 固定幅のなめらかなフォント（32 ビットのウィンドウだけ）
    Smooth(&'static VFont<'static>),
}

impl TermFont {
    fn width(&self) -> usize {
        match self {
            TermFont::Bitmap(f) => f.width,
            TermFont::Smooth(f) => f.cell_width as usize,
        }
    }

    fn height(&self) -> usize {
        match self {
            TermFont::Bitmap(f) => f.height,
            TermFont::Smooth(f) => f.line_height as usize,
        }
    }
}

pub struct Terminal {
    /// 端末のウィンドウ（端末の番号としても使う）
    win: WinId,
    /// 人に見せる番号（1〜）。Alt+F1〜F4 で選ぶ
    number: usize,
    font: TermFont,
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
    /// 描いた文字（cols × rows）。まだ作っていなければ空
    grid: Vec<Cell>,
    /// マウスで選んでいる範囲（格子の番号。始まりと今の端）
    sel: Option<(usize, usize)>,
    /// ボタンを押したまま選んでいる途中
    selecting: bool,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: u8,
    bg: u8,
}

impl Terminal {
    const fn new(win: WinId, number: usize) -> Terminal {
        Terminal {
            win,
            number,
            font: TermFont::Bitmap(&FONT_8X8),
            cols: 0,
            rows: 0,
            col: 0,
            row: 0,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
            parser: Parser::new(),
            cursor_drawn: false,
            grid: Vec::new(),
            sel: None,
            selecting: false,
        }
    }

    /// 大きさをウィンドウに合わせ、先頭から書き始める（画面は消さない）
    fn fit(&mut self, screen: &Screen) {
        self.font = match crate::gfx::fonts::mono() {
            Some(f) if screen.window_is_rgb(self.win) => TermFont::Smooth(f),
            _ => TermFont::Bitmap(&FONT_8X8),
        };
        let (w, h) = screen.window_size(self.win).unwrap_or((0, 0));
        self.cols = w / self.font.width();
        self.rows = h / self.font.height();
        self.col = 0;
        self.row = 0;
        self.cursor_drawn = false;
        self.grid.clear();
        self.sel = None;
        self.selecting = false;
    }

    /// 格子が無ければ作る（ヒープが使えるようになってから）
    fn ensure_grid(&mut self) {
        if self.grid.len() != self.cols * self.rows && sched::is_started() {
            self.grid = vec![Cell { ch: ' ', fg: DEFAULT_FG, bg: DEFAULT_BG }; self.cols * self.rows];
        }
    }

    fn write(&mut self, screen: &mut Screen, args: fmt::Arguments) {
        if self.cols == 0 || self.rows == 0 {
            return; // まだ初期化されていない
        }
        self.ensure_grid();
        // 文字が流れると選んだ位置がずれるので、選択は消す
        self.clear_selection(screen);
        self.hide_cursor(screen);
        let _ = Writer { console: self, screen }.write_fmt(args);
        self.show_cursor(screen);
        screen.present();
    }
}

/// 最初の端末。SCREEN と両方取るときはこちらを先に取ること
static CONSOLE: IrqMutex<Terminal> = IrqMutex::new(Terminal::new(gui::CONSOLE, 1));
/// 2 つ目からの端末（ウィンドウ → 端末）。SCREEN と両方取るときはこちらを先に取ること。
/// CONSOLE と同時には取らない
static TERMINALS: IrqMutex<BTreeMap<WinId, Box<Terminal>>> = IrqMutex::new(BTreeMap::new());

/// 最初の端末の大きさを今のウィンドウに合わせ、先頭から書き始める（画面は消さない）
pub fn init() {
    let mut c = CONSOLE.lock();
    let s = SCREEN.lock();
    c.fit(&s);
}

/// 最初の端末の文字色と背景色（パレット番号）を直接設定する
pub fn set_color(fg: u8, bg: u8) {
    let mut c = CONSOLE.lock();
    c.fg = fg;
    c.bg = bg;
}

/// 今のタスクの端末に書く。端末が閉じていれば捨てる（シリアルには print が出している）
pub fn write_fmt(args: fmt::Arguments) {
    write_to(sched::current_tty(), args);
}

pub fn write_to(tty: WinId, args: fmt::Arguments) {
    if tty == gui::CONSOLE {
        let mut c = CONSOLE.lock();
        let mut s = SCREEN.lock();
        c.write(&mut s, args);
    } else {
        let mut terms = TERMINALS.lock();
        if let Some(t) = terms.get_mut(&tty) {
            let mut s = SCREEN.lock();
            t.write(&mut s, args);
        }
    }
}

/// 今のタスクの端末の大きさ（桁, 行）。端末が無ければ None
pub fn size() -> Option<(usize, usize)> {
    let tty = sched::current_tty();
    if tty == gui::CONSOLE {
        let c = CONSOLE.lock();
        Some((c.cols, c.rows))
    } else {
        TERMINALS.lock().get(&tty).map(|t| (t.cols, t.rows))
    }
}

/// 使っている番号
fn numbers_in_use() -> [bool; 64] {
    let mut used = [false; 64];
    used[1] = true; // 最初の端末
    for t in TERMINALS.lock().values() {
        if t.number < used.len() {
            used[t.number] = true;
        }
    }
    used
}

/// 番号 `n` の端末のウィンドウ
fn find_number(n: usize) -> Option<WinId> {
    if n == 1 {
        return Some(gui::CONSOLE);
    }
    TERMINALS.lock().values().find(|t| t.number == n).map(|t| t.win)
}

/// 新しい端末を開いてシェルを動かす。`number` を指定しなければ空いている一番小さい番号。
/// `command` があれば、シェルはそれを実行して終わり、端末を閉じる（タイトルはコマンド）。
/// 開いた端末のウィンドウを返す
pub fn open_terminal(number: Option<usize>, command: Option<alloc::string::String>) -> Option<WinId> {
    let used = numbers_in_use();
    let n = match number {
        Some(n) if n < used.len() && !used[n] => n,
        Some(_) => return None,
        None => (2..used.len()).find(|&n| !used[n])?,
    };
    let mut term = Box::new(Terminal::new(0, n));
    let win = {
        let mut terms = TERMINALS.lock();
        let mut s = SCREEN.lock();
        let title = match &command {
            Some(c) => c.clone(),
            None => format!("Terminal {n}"),
        };
        let win = s.create_terminal(&title);
        term.win = win;
        term.fit(&s);
        s.present();
        terms.insert(win, term);
        win
    };
    klog!("term: opened terminal {n} (window {win})");
    sched::spawn(&format!("shell{n}"), move || {
        sched::set_current_tty(win);
        crate::shell::run_terminal(command);
        close_terminal(win);
    });
    Some(win)
}

/// 端末を片付ける（シェルが終わったとき）。最初の端末は閉じない
fn close_terminal(win: WinId) {
    let term = TERMINALS.lock().remove(&win);
    if let Some(t) = term {
        klog!("term: closed terminal {}", t.number);
    }
    let mut s = SCREEN.lock();
    s.destroy_window(win);
    s.present();
}

/// gui から: 端末のウィンドウ `tty` の中身でのマウス（選んでクリップボードへ）
pub fn mouse(tty: WinId, x: i32, y: i32, buttons: u8) {
    if tty == gui::CONSOLE {
        let mut c = CONSOLE.lock();
        let mut s = SCREEN.lock();
        c.mouse(&mut s, x, y, buttons);
    } else {
        let mut terms = TERMINALS.lock();
        if let Some(t) = terms.get_mut(&tty) {
            let mut s = SCREEN.lock();
            t.mouse(&mut s, x, y, buttons);
        }
    }
}

/// Alt+F1〜F4: 番号 `n` の端末（とそれに属するウィンドウ）を前に出す。無ければ開く
pub fn switch_to(n: usize) {
    if n == 0 || n > MAX_NUMBERED {
        return;
    }
    match find_number(n) {
        Some(win) => {
            let mut s = SCREEN.lock();
            s.raise_tty(win);
            s.present();
        }
        None => {
            open_terminal(Some(n), None);
        }
    }
}

struct Writer<'a> {
    console: &'a mut Terminal,
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

impl Terminal {
    fn cell_y(&self, row: usize) -> usize {
        row * self.font.height()
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
        if let Some(c) = self.grid.get_mut(row * self.cols + col) {
            *c = Cell { ch, fg: self.fg, bg: self.bg };
        }
        let (x, y) = (col * self.font.width(), self.cell_y(row));
        self.paint_char(screen, x, y, ch, self.fg, self.bg);
        screen.mark_window_dirty(self.win, y, y + self.font.height());
    }

    /// 文字の枠 (x, y) に 1 文字描く（背景も塗る）
    fn paint_char(&self, screen: &mut Screen, x: usize, y: usize, ch: char, fg: u8, bg: u8) {
        let (x, y) = (x as i32, y as i32);
        let (w, h) = (self.font.width() as i32, self.font.height() as i32);
        let Some(px) = screen.window_pixels(self.win) else { return };
        match (px, self.font) {
            (WinPixels::P8(mut s), TermFont::Bitmap(f)) => s.draw_char(f, x, y, ch, fg, Some(bg)),
            (WinPixels::P8(mut s), TermFont::Smooth(_)) => s.draw_char(&FONT_8X8, x, y, ch, fg, Some(bg)),
            (WinPixels::P32(mut c, lut), TermFont::Bitmap(f)) => {
                c.draw_char(f, x, y, ch, lut[fg as usize], Some(lut[bg as usize]))
            }
            (WinPixels::P32(mut c, lut), TermFont::Smooth(f)) => {
                c.fill_rect(x, y, w, h, lut[bg as usize]);
                c.draw_glyph(f, x, y, ch, lut[fg as usize]);
            }
        }
    }

    fn erase_cells(&mut self, screen: &mut Screen, row: usize, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (w, h) = (self.font.width(), self.font.height());
        let y = self.cell_y(row);
        self.blank_grid(row * self.cols + from, row * self.cols + to);
        if let Some(mut px) = screen.window_pixels(self.win) {
            px.fill_rect((from * w) as i32, y as i32, ((to - from) * w) as i32, h as i32, self.bg);
        }
        screen.mark_window_dirty(self.win, y, y + h);
    }

    fn erase_rows(&mut self, screen: &mut Screen, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let (y0, y1) = (self.cell_y(from), self.cell_y(to));
        self.blank_grid(from * self.cols, to * self.cols);
        if let Some(mut px) = screen.window_pixels(self.win) {
            px.fill_rect(0, y0 as i32, (self.cols * self.font.width()) as i32, (y1 - y0) as i32, self.bg);
        }
        screen.mark_window_dirty(self.win, y0, y1);
    }

    fn next_line(&mut self, screen: &mut Screen) {
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        let h = self.font.height() as i32;
        if !self.grid.is_empty() {
            self.grid.copy_within(self.cols.., 0);
            let n = self.grid.len();
            self.blank_grid(n - self.cols, n);
        }
        let (y0, y1) = (self.cell_y(0), self.cell_y(self.rows));
        if let Some(mut px) = screen.window_pixels(self.win) {
            px.scroll_up(0, y0 as i32, (self.cols * self.font.width()) as i32, (y1 - y0) as i32, h, self.bg);
        }
        screen.mark_window_dirty(self.win, y0, y1);
    }

    /// カーソル（文字の下の 1 ピクセルの線）
    fn cursor_line(&self) -> (i32, i32, i32) {
        let col = self.col.min(self.cols - 1);
        let x = (col * self.font.width()) as i32;
        let y = (self.cell_y(self.row) + self.font.height() - 1) as i32;
        (x, y, self.font.width() as i32)
    }

    fn show_cursor(&mut self, screen: &mut Screen) {
        let (x, y, w) = self.cursor_line();
        if let Some(mut px) = screen.window_pixels(self.win) {
            px.hline(x, y, w, self.fg);
        }
        screen.mark_window_dirty(self.win, y as usize, y as usize + 1);
        self.cursor_drawn = true;
    }

    fn hide_cursor(&mut self, screen: &mut Screen) {
        if !self.cursor_drawn {
            return;
        }
        let (x, y, w) = self.cursor_line();
        if let Some(mut px) = screen.window_pixels(self.win) {
            px.hline(x, y, w, self.bg);
        }
        screen.mark_window_dirty(self.win, y as usize, y as usize + 1);
        self.cursor_drawn = false;
    }

    /// 格子の [from, to) を今の背景色の空白にする
    fn blank_grid(&mut self, from: usize, to: usize) {
        let bg = self.bg;
        let to = to.min(self.grid.len());
        if from < to {
            self.grid[from..to].fill(Cell { ch: ' ', fg: DEFAULT_FG, bg });
        }
    }

    /// 格子の `i` 番目を描き直す（`inverted` なら文字色と背景色を入れ替える）
    fn repaint_cell(&self, screen: &mut Screen, i: usize, inverted: bool) {
        let Some(c) = self.grid.get(i) else { return };
        let (fg, bg) = if inverted { (c.bg, c.fg) } else { (c.fg, c.bg) };
        let (x, y) = ((i % self.cols) * self.font.width(), self.cell_y(i / self.cols));
        self.paint_char(screen, x, y, c.ch, fg, bg);
        screen.mark_window_dirty(self.win, y, y + self.font.height());
    }

    fn paint_selection(&self, screen: &mut Screen, inverted: bool) {
        if let Some((a, b)) = self.sel {
            for i in a.min(b)..=a.max(b) {
                self.repaint_cell(screen, i, inverted);
            }
        }
    }

    fn clear_selection(&mut self, screen: &mut Screen) {
        if self.sel.is_some() {
            self.paint_selection(screen, false);
            self.sel = None;
            // カーソルの線も消えたかもしれないので描き直す
            if self.cursor_drawn {
                self.show_cursor(screen);
            }
        }
        self.selecting = false;
    }

    /// 選んだ範囲の文字（行ごとに末尾の空白を除き、改行でつなぐ）
    fn selected_text(&self) -> String {
        let Some((a, b)) = self.sel else { return String::new() };
        let (lo, hi) = (a.min(b), a.max(b));
        let mut out = String::new();
        let mut row = lo / self.cols;
        let mut line = String::new();
        for i in lo..=hi {
            if i / self.cols != row {
                out.push_str(line.trim_end());
                out.push('\n');
                line.clear();
                row = i / self.cols;
            }
            line.push(self.grid[i].ch);
        }
        out.push_str(line.trim_end());
        out
    }

    /// マウス（中身の座標）。押してなぞると選び、離すとクリップボードに入れる
    fn mouse(&mut self, screen: &mut Screen, x: i32, y: i32, buttons: u8) {
        if self.grid.is_empty() {
            return;
        }
        let col = (x.max(0) as usize / self.font.width()).min(self.cols - 1);
        let row = (y.max(0) as usize / self.font.height()).min(self.rows - 1);
        let cell = row * self.cols + col;
        let left = buttons & 1 != 0;
        match (left, self.selecting) {
            (true, false) => {
                self.clear_selection(screen);
                self.sel = Some((cell, cell));
                self.selecting = true;
                self.paint_selection(screen, true);
            }
            (true, true) => {
                if let Some((start, end)) = self.sel {
                    if end != cell {
                        self.paint_selection(screen, false);
                        self.sel = Some((start, cell));
                        self.paint_selection(screen, true);
                    }
                }
            }
            (false, true) => {
                self.selecting = false;
                match self.sel {
                    // 動かさずに離したら選ばない
                    Some((a, b)) if a == b => self.clear_selection(screen),
                    _ => {
                        let text = self.selected_text();
                        klog!("term: copied {} bytes", text.len());
                        crate::clipboard::set(&text);
                    }
                }
            }
            (false, false) => {}
        }
        screen.present();
    }
}
