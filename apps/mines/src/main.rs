//! マインスイーパ（ウィンドウ）。9 × 9 のマスに地雷が 10 個
//!
//! 左クリックで開く、右クリックで旗。上の顔を押すか N で新しいゲーム、Esc か [x] で終わる。
//! 引数で乱数の種を指定できる（同じ種なら同じ盤面。テスト用）。中身は `vibe_games::mines`。

#![no_std]
#![no_main]

use vibe_games::mines::{Mines, State};
use vibe_rt::abi::key;
use vibe_rt::gfx::{Event, Window, draw_text16_centered, font16};
use vibe_rt::gfx_lib::Surface;
use vibe_rt::gfx_lib::palette::{BLACK, DARK_GRAY, WHITE};
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

/// 開いたマスの地と区切りの線（濃い灰の 1 ピクセルの線は画面では点線）。白黒の画面（docs/plans/mono.md）
/// なので、数字は全部黒の太字、負けたマスは黒い地に白い地雷
const OPENED: u8 = WHITE;
const GRID: u8 = DARK_GRAY;
const LOST: u8 = BLACK;

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

/// 開いていないマス・顔のボタン: 黒い枠の白い箱と、右下に 1 ピクセルずらした黒い影
fn raised(s: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    s.fill_rect(x + 1, y + 1, w - 1, h - 1, BLACK);
    s.fill_rect(x, y, w - 1, h - 1, WHITE);
    s.rect(x, y, w - 1, h - 1, BLACK);
}

/// 3 桁の数を黒い角丸に白い太字で出す
fn counter(s: &mut Surface, x: i32, value: i32) {
    s.fill_round_rect(x, MARGIN, 40, PANEL_H, 4, BLACK);
    let v = value.clamp(-99, 999);
    let text = format_fixed::<8>(format_args!("{v:03}"));
    let top = MARGIN + (PANEL_H - font16().height as i32) / 2;
    draw_text16_centered(s, x, 40, top, &text, WHITE, true);
}

/// 顔のボタン（白い箱に、白い丸に目と口）
fn face(s: &mut Surface, state: State) {
    raised(s, FACE_X, MARGIN, 24, PANEL_H);
    let (cx, cy) = (FACE_X + 11, MARGIN + 11);
    s.circle(cx, cy, 7, BLACK);
    let ink = BLACK;
    match state {
        State::Won => {
            // サングラス
            s.fill_rect(cx - 5, cy - 3, 4, 3, ink);
            s.fill_rect(cx + 1, cy - 3, 4, 3, ink);
            s.hline(cx - 1, cy - 3, 2, ink);
        }
        State::Lost(..) => {
            for ex in [cx - 3, cx + 3] {
                s.line(ex - 1, cy - 4, ex + 1, cy - 2, ink);
                s.line(ex + 1, cy - 4, ex - 1, cy - 2, ink);
            }
        }
        _ => {
            for ex in [cx - 3, cx + 3] {
                s.fill_rect(ex - 1, cy - 3, 2, 2, ink);
            }
        }
    }
    // 口: 勝ち・遊んでいるときは笑顔、負けたら逆さ
    if matches!(state, State::Lost(..)) {
        s.hline(cx - 2, cy + 2, 5, ink);
        s.put_pixel(cx - 3, cy + 3, ink);
        s.put_pixel(cx + 3, cy + 3, ink);
    } else {
        s.hline(cx - 2, cy + 4, 5, ink);
        s.put_pixel(cx - 3, cy + 3, ink);
        s.put_pixel(cx + 3, cy + 3, ink);
    }
}

fn draw(s: &mut Surface, g: &Game) {
    s.clear(WHITE);
    counter(s, MARGIN, g.mines.mines_left());
    counter(s, W as i32 - MARGIN - 40, g.seconds().min(999) as i32);
    face(s, g.mines.state());
    for cy in 0..ROWS {
        for cx in 0..COLS {
            let (x, y) = (BOARD_X + cx as i32 * CELL, BOARD_Y + cy as i32 * CELL);
            let cell = g.mines.cell(cx, cy);
            if !cell.open {
                raised(s, x, y, CELL, CELL);
                if cell.flag {
                    // 旗: 黒い棒と台、黒い三角
                    s.vline(x + 8, y + 3, 9, BLACK);
                    for (i, w) in [2, 3, 5, 3, 2].into_iter().enumerate() {
                        s.hline(x + 8 - w, y + 3 + i as i32, w, BLACK);
                    }
                    s.hline(x + 4, y + 12, 8, BLACK);
                }
                continue;
            }
            let lost_here = g.mines.state() == State::Lost(cx, cy);
            s.fill_rect(x, y, CELL, CELL, if lost_here { LOST } else { OPENED });
            // 開いたマスは右と下に区切りの線
            s.hline(x, y + CELL - 1, CELL, GRID);
            s.vline(x + CELL - 1, y, CELL, GRID);
            if cell.mine {
                let ink = if lost_here { WHITE } else { BLACK };
                for (dx, dy) in [(5, 0), (0, 5), (4, 4), (4, -4)] {
                    s.line(x + 7 - dx, y + 7 - dy, x + 7 + dx, y + 7 + dy, ink);
                }
                s.fill_circle(x + 7, y + 7, 4, ink);
                s.fill_rect(x + 5, y + 5, 2, 2, if lost_here { BLACK } else { WHITE });
            } else if cell.around > 0 {
                let ch = (b'0' + cell.around) as char;
                let text = format_fixed::<2>(format_args!("{ch}"));
                draw_text16_centered(s, x, CELL - 1, y, &text, BLACK, true);
            }
        }
    }
}
