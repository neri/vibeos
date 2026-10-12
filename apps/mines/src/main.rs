//! マインスイーパ（ウィンドウ）。9 × 9 のマスに地雷が 10 個
//!
//! 左クリックで開く、右クリックで旗。上の顔を押すか N で新しいゲーム、Esc か [x] で終わる。
//! 引数で乱数の種を指定できる（同じ種なら同じ盤面。テスト用）。中身は `vibe_games::mines`。

#![no_std]
#![no_main]

use vibe_games::mines::{Mines, State};
use vibe_rt::abi::key;
use vibe_rt::font::FONT_8X8;
use vibe_rt::gfx::{Event, Window};
use vibe_rt::gfx_lib::{Surface, palette};
use vibe_rt::{format_fixed, println, sys};

vibe_rt::entry!(main);

const COLS: usize = 9;
const ROWS: usize = 9;
const MINES: usize = 10;
const CELL: i32 = 16;
const MARGIN: i32 = 6;
const PANEL_H: i32 = 24;
const BOARD_X: i32 = MARGIN;
const BOARD_Y: i32 = MARGIN + PANEL_H + MARGIN;
const W: usize = (COLS as i32 * CELL + 2 * MARGIN) as usize;
const H: usize = (BOARD_Y + ROWS as i32 * CELL + MARGIN) as usize;
const FACE_X: i32 = W as i32 / 2 - 12;

/// 数字の色（1〜8）
const NUMBER_COLORS: [u8; 8] = [
    palette::BLUE,
    palette::GREEN,
    palette::RED,
    palette::CUBE_BASE + 2, // 濃い青
    palette::BROWN,
    palette::CYAN,
    palette::BLACK,
    palette::DARK_GRAY,
];

struct Game {
    mines: Mines,
    /// 最初に開いた時刻（刻み）と、終わった時刻
    started: Option<u32>,
    ended: Option<u32>,
}

impl Game {
    fn new(seed: u32) -> Game {
        Game { mines: Mines::new(COLS, ROWS, MINES, seed), started: None, ended: None }
    }

    fn seconds(&self) -> u32 {
        let Some(s) = self.started else { return 0 };
        (self.ended.unwrap_or_else(sys::ticks) - s) / 100
    }
}

fn main() -> i32 {
    let mut seed = vibe_rt::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(sys::ticks);
    let Some(mut win) = Window::open(W, H, "Mines") else {
        println!("mines: cannot open a window");
        return 1;
    };
    let mut game = Game::new(seed);
    println!("mines: started (seed {seed})");
    let mut prev_buttons = 0;
    loop {
        draw(&mut win.surface(), &game);
        win.present();
        // 遊んでいる間は時間の表示のために 1 秒ごとに描き直す
        let timeout = if game.started.is_some() && game.ended.is_none() { Some(1000) } else { None };
        match win.event(timeout) {
            Some(Event::Close) => break,
            Some(Event::Key(k)) if k.pressed() && k.key() == key::ESCAPE => break,
            Some(Event::Key(k)) if k.pressed() && matches!(k.ch(), Some('n' | 'N')) => {
                seed = seed.wrapping_add(1);
                game = Game::new(seed);
                println!("mines: new game");
            }
            Some(Event::Mouse { x, y, buttons }) => {
                let pressed = buttons & !prev_buttons;
                prev_buttons = buttons;
                if pressed & 1 != 0 && (FACE_X..FACE_X + 24).contains(&x) && (MARGIN..MARGIN + PANEL_H).contains(&y) {
                    seed = seed.wrapping_add(1);
                    game = Game::new(seed);
                    println!("mines: new game");
                    continue;
                }
                let (cx, cy) = ((x - BOARD_X).div_euclid(CELL), (y - BOARD_Y).div_euclid(CELL));
                if !(0..COLS as i32).contains(&cx) || !(0..ROWS as i32).contains(&cy) {
                    continue;
                }
                let (cx, cy) = (cx as usize, cy as usize);
                if pressed & 1 != 0 && game.mines.open(cx, cy) {
                    game.started.get_or_insert_with(sys::ticks);
                } else if pressed & 2 != 0 {
                    game.mines.toggle_flag(cx, cy);
                }
                if game.mines.is_over() && game.ended.is_none() {
                    game.ended = Some(sys::ticks());
                    match game.mines.state() {
                        State::Won => println!("mines: won in {} s", game.seconds()),
                        _ => println!("mines: lost"),
                    }
                }
            }
            _ => {}
        }
    }
    drop(win);
    println!("mines: closed");
    0
}

/// 立体の枠（出っ張り）
fn raised(s: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    s.fill_rect(x, y, w, h, palette::LIGHT_GRAY);
    s.hline(x, y, w, palette::WHITE);
    s.vline(x, y, h, palette::WHITE);
    s.hline(x, y + h - 1, w, palette::DARK_GRAY);
    s.vline(x + w - 1, y, h, palette::DARK_GRAY);
}

/// 3 桁の数を黒地に赤で出す
fn counter(s: &mut Surface, x: i32, value: i32) {
    s.fill_rect(x, MARGIN, 40, PANEL_H, palette::BLACK);
    let v = value.clamp(-99, 999);
    let text = format_fixed::<8>(format_args!("{v:03}"));
    s.draw_text(&FONT_8X8, x + 8, MARGIN + 8, &text, palette::LIGHT_RED, None);
}

fn draw(s: &mut Surface, g: &Game) {
    s.clear(palette::LIGHT_GRAY);
    counter(s, MARGIN, g.mines.mines_left());
    counter(s, W as i32 - MARGIN - 40, g.seconds().min(999) as i32);
    raised(s, FACE_X, MARGIN, 24, PANEL_H);
    let face = match g.mines.state() {
        State::Won => "B)",
        State::Lost(..) => ":(",
        _ => ":)",
    };
    s.draw_text(&FONT_8X8, FACE_X + 4, MARGIN + 8, face, palette::BLACK, None);
    for cy in 0..ROWS {
        for cx in 0..COLS {
            let (x, y) = (BOARD_X + cx as i32 * CELL, BOARD_Y + cy as i32 * CELL);
            let c = g.mines.cell(cx, cy);
            if !c.open {
                raised(s, x, y, CELL, CELL);
                if c.flag {
                    // 旗: 棒の左に赤い三角、下に台
                    s.vline(x + 9, y + 3, 9, palette::BLACK);
                    s.hline(x + 5, y + 12, 7, palette::BLACK);
                    for (i, w) in [2, 3, 5, 3, 2].into_iter().enumerate() {
                        s.hline(x + 9 - w, y + 3 + i as i32, w, palette::RED);
                    }
                }
                continue;
            }
            let lost_here = g.mines.state() == State::Lost(cx, cy);
            s.fill_rect(x, y, CELL, CELL, if lost_here { palette::RED } else { palette::LIGHT_GRAY });
            s.hline(x, y, CELL, palette::DARK_GRAY);
            s.vline(x, y, CELL, palette::DARK_GRAY);
            if c.mine {
                s.fill_rect(x + 4, y + 4, 8, 8, palette::BLACK);
                s.hline(x + 2, y + 8, 12, palette::BLACK);
                s.vline(x + 8, y + 2, 12, palette::BLACK);
                s.put_pixel(x + 6, y + 6, palette::WHITE);
            } else if c.around > 0 {
                let ch = (b'0' + c.around) as char;
                s.draw_char(&FONT_8X8, x + 4, y + 4, ch, NUMBER_COLORS[c.around as usize - 1], None);
            }
        }
    }
}
