//! マインスイーパ（ウィンドウ）。9 × 9 のマスに地雷が 10 個
//!
//! 左クリックで開く、右クリックで旗。上の顔を押すか N で新しいゲーム、Esc か [x] で終わる。
//! 引数で乱数の種を指定できる（同じ種なら同じ盤面。テスト用）。中身は `vibe_games::mines`。

#![no_std]
#![no_main]

use vibe_games::mines::{Mines, State};
use vibe_rt::abi::key;
use vibe_rt::abi::{FONT_BOLD, FONT_MONO};
use vibe_rt::gfx::{Event, Window32 as Window, draw_text_centered, text_height};
use vibe_rt::gfx_lib::canvas::{Canvas, Color, Corner, rgb};
use vibe_rt::gfx_lib::theme;
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
const NUMBER_COLORS: [Color; 8] = [
    rgb(37, 99, 235),
    rgb(22, 150, 70),
    rgb(220, 38, 38),
    rgb(30, 58, 160),
    rgb(146, 64, 14),
    rgb(13, 148, 136),
    rgb(32, 33, 36),
    rgb(110, 114, 122),
];
/// まだ開いていないマス・開いたマス・負けたマスの地
const CLOSED: Color = rgb(190, 197, 209);
const CLOSED_TOP: Color = rgb(206, 212, 222);
const OPENED: Color = rgb(236, 238, 242);
const LOST: Color = rgb(239, 68, 68);

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
        draw(&mut win.canvas(), &game);
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

/// 3 桁の数を濃い角の丸い地に赤で出す
fn counter(c: &mut Canvas, k: &Corner, x: i32, value: i32) {
    c.fill_round_rect(x, MARGIN, 40, PANEL_H, k, rgb(32, 33, 36), 255);
    let v = value.clamp(-99, 999);
    let text = format_fixed::<8>(format_args!("{v:03}"));
    let top = MARGIN + (PANEL_H - text_height(FONT_MONO)) / 2;
    draw_text_centered(c, FONT_MONO, x, 40, top, &text, rgb(248, 113, 113));
}

/// 顔のボタン（黄色の丸に目と口）
fn face(c: &mut Canvas, k: &Corner, state: State) {
    c.fill_round_rect(FACE_X, MARGIN, 24, PANEL_H, k, rgb(255, 255, 255), 255);
    c.round_rect(FACE_X, MARGIN, 24, PANEL_H, k, rgb(198, 201, 208));
    let (cx, cy) = (FACE_X + 12, MARGIN + 12);
    c.fill_round_rect(cx - 8, cy - 8, 16, 16, &Corner::new(8), rgb(250, 204, 21), 255);
    let ink = theme::TEXT;
    let p = |v: i32| v * 16 + 8;
    match state {
        State::Won => {
            // サングラス
            c.fill_rect(cx - 6, cy - 3, 5, 3, ink);
            c.fill_rect(cx + 1, cy - 3, 5, 3, ink);
            c.hline(cx - 1, cy - 3, 2, ink);
        }
        State::Lost(..) => {
            for ex in [cx - 4, cx + 3] {
                c.line_aa(p(ex) - 20, p(cy - 3) - 20, p(ex) + 20, p(cy - 3) + 20, 16, ink);
                c.line_aa(p(ex) + 20, p(cy - 3) - 20, p(ex) - 20, p(cy - 3) + 20, 16, ink);
            }
        }
        _ => {
            for ex in [cx - 4, cx + 3] {
                c.fill_round_rect(ex - 1, cy - 4, 3, 3, &Corner::new(1), ink, 255);
            }
        }
    }
    // 口: 勝ち・遊んでいるときは笑顔、負けたら逆さ
    let (y0, y1) = if matches!(state, State::Lost(..)) { (cy + 5, cy + 3) } else { (cy + 3, cy + 5) };
    c.line_aa(p(cx - 4), p(y0), p(cx), p(y1), 18, ink);
    c.line_aa(p(cx), p(y1), p(cx + 4), p(y0), 18, ink);
}

fn draw(c: &mut Canvas, g: &Game) {
    let k = Corner::new(4);
    let k_cell = Corner::new(3);
    c.clear(theme::FACE);
    counter(c, &k, MARGIN, g.mines.mines_left());
    counter(c, &k, W as i32 - MARGIN - 40, g.seconds().min(999) as i32);
    face(c, &k, g.mines.state());
    let p = |v: i32| v * 16 + 8;
    for cy in 0..ROWS {
        for cx in 0..COLS {
            let (x, y) = (BOARD_X + cx as i32 * CELL, BOARD_Y + cy as i32 * CELL);
            let cell = g.mines.cell(cx, cy);
            if !cell.open {
                // 少し盛り上がった角の丸いマス（上半分を明るく）
                c.fill_round_rect(x + 1, y + 1, CELL - 2, CELL - 2, &k_cell, CLOSED, 255);
                c.fill_round_rect(x + 2, y + 2, CELL - 4, CELL / 2 - 2, &Corner::new(2), CLOSED_TOP, 255);
                if cell.flag {
                    // 旗: 棒と赤い三角
                    c.line_aa(p(x + 9), p(y + 3), p(x + 9), p(y + 12), 18, theme::TEXT);
                    for (i, w) in [2, 3, 5, 3, 2].into_iter().enumerate() {
                        c.hline(x + 9 - w, y + 3 + i as i32, w, rgb(220, 38, 38));
                    }
                    c.hline(x + 5, y + 12, 8, theme::TEXT);
                }
                continue;
            }
            let lost_here = g.mines.state() == State::Lost(cx, cy);
            c.fill_rect(x, y, CELL, CELL, if lost_here { LOST } else { OPENED });
            if cell.mine {
                let ink = theme::TEXT;
                for (dx, dy) in [(5, 0), (0, 5), (4, 4), (4, -4)] {
                    c.line_aa(p(x + 8 - dx), p(y + 8 - dy), p(x + 8 + dx), p(y + 8 + dy), 16, ink);
                }
                c.fill_round_rect(x + 4, y + 4, 9, 9, &Corner::new(4), ink, 255);
                c.put_pixel(x + 6, y + 6, rgb(255, 255, 255));
            } else if cell.around > 0 {
                let ch = (b'0' + cell.around) as char;
                let text = format_fixed::<2>(format_args!("{ch}"));
                let top = y + (CELL - text_height(FONT_BOLD)) / 2;
                draw_text_centered(c, FONT_BOLD, x, CELL, top, &text, NUMBER_COLORS[cell.around as usize - 1]);
            }
        }
    }
}
