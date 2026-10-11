//! テキストコンソール（画面全体を文字の格子として使う）

use core::fmt::{self, Write};

use vibe_font::{FONT_8X8, Font};
use vibe_gfx::palette;

use crate::drivers::vga::{self, SCREEN, Screen};
use crate::sync::IrqMutex;

const TAB_WIDTH: usize = 4;

pub struct Console {
    font: &'static Font,
    cols: usize,
    rows: usize,
    col: usize,
    row: usize,
    fg: u8,
    bg: u8,
}

/// コンソール。SCREEN と両方取るときはこちらを先に取ること
static CONSOLE: IrqMutex<Console> = IrqMutex::new(Console {
    font: &FONT_8X8,
    cols: 0,
    rows: 0,
    col: 0,
    row: 0,
    fg: palette::LIGHT_GRAY,
    bg: palette::BLACK,
});

pub fn init() {
    let mut c = CONSOLE.lock();
    c.cols = vga::WIDTH / c.font.width;
    c.rows = vga::HEIGHT / c.font.height;
    c.col = 0;
    c.row = 0;
}

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
    let _ = Writer { console: &mut console, screen: &mut screen }.write_fmt(args);
    screen.present();
}

struct Writer<'a> {
    console: &'a mut Console,
    screen: &'a mut Screen,
}

impl Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            self.console.put_char(self.screen, ch);
        }
        Ok(())
    }
}

impl Console {
    fn put_char(&mut self, screen: &mut Screen, ch: char) {
        match ch {
            '\n' => self.newline(screen),
            '\r' => self.col = 0,
            '\t' => {
                let next = (self.col / TAB_WIDTH + 1) * TAB_WIDTH;
                while self.col < next.min(self.cols) {
                    self.put_char(screen, ' ');
                }
            }
            '\x08' => {
                if self.col > 0 {
                    self.col -= 1;
                    self.draw(screen, ' ');
                }
            }
            _ => {
                if self.col >= self.cols {
                    self.newline(screen);
                }
                self.draw(screen, ch);
                self.col += 1;
            }
        }
    }

    fn draw(&mut self, screen: &mut Screen, ch: char) {
        let (w, h) = (self.font.width, self.font.height);
        let (x, y) = (self.col * w, self.row * h);
        screen.surface().draw_char(self.font, x as i32, y as i32, ch, self.fg, Some(self.bg));
        screen.mark_dirty(y, y + h);
    }

    fn newline(&mut self, screen: &mut Screen) {
        self.col = 0;
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        let h = self.font.height as i32;
        let height = (self.rows * self.font.height) as i32;
        screen.surface().scroll_up(0, 0, vga::WIDTH as i32, height, h, self.bg);
        screen.mark_dirty(0, height as usize);
    }
}
