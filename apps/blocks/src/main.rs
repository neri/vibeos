//! 落ち物パズル（ウィンドウ）
//!
//! ← → で動かす、↑ で回す、↓ でゆっくり落とす、Space で一気に落とす、P で一時停止、
//! 終わったら Enter でもう一度、Esc か [x] で終わる。列が消えると音が鳴る。
//! 引数で乱数の種を指定できる（同じ種なら同じ順にブロックが出る。テスト用）。中身は `vibe_games::blocks`。

#![no_std]
#![no_main]

use vibe_games::blocks::{self, Blocks, Move};
use vibe_rt::abi::key;
use vibe_rt::abi::{FONT_BOLD, FONT_UI};
use vibe_rt::gfx::{Event, Window32 as Window, draw_text, draw_text_centered, text_height};
use vibe_rt::gfx_lib::canvas::{Canvas, Color, Corner, rgb};
use vibe_rt::gfx_lib::theme;
use vibe_rt::{format_fixed, println, sys};

vibe_rt::entry!(main);

const CELL: i32 = 8;
const BOARD_X: i32 = 8;
const BOARD_Y: i32 = 8;
const PANEL_X: i32 = BOARD_X + blocks::W as i32 * CELL + 10;
const W: usize = 152;
const H: usize = (BOARD_Y * 2 + blocks::H as i32 * CELL) as usize;

/// 形ごとの色（I O T S Z J L）
const COLORS: [Color; 7] = [
    rgb(34, 211, 238),
    rgb(250, 204, 21),
    rgb(192, 132, 252),
    rgb(74, 222, 128),
    rgb(248, 113, 113),
    rgb(96, 165, 250),
    rgb(251, 146, 60),
];
/// 盤面の地と、止まる位置の影
const BOARD: Color = rgb(30, 32, 38);
const GHOST: Color = rgb(90, 95, 106);

fn main() -> i32 {
    let mut seed = vibe_rt::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(sys::ticks);
    let Some(mut win) = Window::open(W, H, "Blocks") else {
        println!("blocks: cannot open a window");
        return 1;
    };
    let mut game = Blocks::new(seed);
    let mut paused = false;
    println!("blocks: started (seed {seed})");
    let mut next_fall = sys::ticks() + game.fall_interval_ms() / 10;
    loop {
        draw(&mut win.canvas(), &game, paused);
        win.present();
        let wait = if game.is_over() || paused { None } else { Some(next_fall.saturating_sub(sys::ticks()) * 10) };
        let mv = match win.event(wait) {
            Some(Event::Close) => break,
            Some(Event::Key(k)) if k.pressed() => match (k.key(), k.ch()) {
                (key::ESCAPE, _) => break,
                (key::LEFT, _) => Some(Move::Left),
                (key::RIGHT, _) => Some(Move::Right),
                (key::UP, _) => Some(Move::Rotate),
                (key::DOWN, _) => Some(Move::Down),
                (key::SPACE, _) => Some(Move::Drop),
                (key::ENTER, _) if game.is_over() => {
                    seed = seed.wrapping_add(1);
                    game = Blocks::new(seed);
                    println!("blocks: new game");
                    None
                }
                (_, Some('p' | 'P')) if !game.is_over() => {
                    paused = !paused;
                    None
                }
                _ => None,
            },
            _ => None,
        };
        if paused {
            next_fall = sys::ticks() + game.fall_interval_ms() / 10;
            continue;
        }
        let was_over = game.is_over();
        let mut outcome = mv.map(|m| game.apply(m)).unwrap_or_default();
        if sys::ticks() >= next_fall && !game.is_over() {
            outcome.lines += game.tick().lines;
            next_fall = sys::ticks() + game.fall_interval_ms() / 10;
        }
        if outcome.lines > 0 {
            sys::sound(440 + 220 * outcome.lines, 80);
        }
        if game.is_over() && !was_over {
            sys::sound(110, 400);
            println!("blocks: game over, score {} lines {}", game.score(), game.lines());
        }
    }
    drop(win);
    println!("blocks: quit, score {} lines {}", game.score(), game.lines());
    0
}

/// 1 マス: 角の丸いブロックに、上の縁を明るく
fn cell(c: &mut Canvas, k: &Corner, x: i32, y: i32, color: Color) {
    let (px, py) = (BOARD_X + x * CELL, BOARD_Y + y * CELL);
    c.fill_round_rect(px, py, CELL - 1, CELL - 1, k, color, 255);
    c.hline(px + 2, py, CELL - 5, vibe_rt::gfx_lib::canvas::blend(color, rgb(255, 255, 255), 110));
}

fn draw(c: &mut Canvas, g: &Blocks, paused: bool) {
    let k = Corner::new(2);
    let panel = Corner::new(4);
    c.clear(theme::FACE);
    let (bw, bh) = (blocks::W as i32 * CELL, blocks::H as i32 * CELL);
    c.fill_round_rect(BOARD_X - 2, BOARD_Y - 2, bw + 4, bh + 4, &panel, BOARD, 255);
    for y in 0..blocks::H {
        for x in 0..blocks::W {
            let v = g.cell(x, y);
            if v != 0 {
                cell(c, &k, x as i32, y as i32, COLORS[v as usize - 1]);
            }
        }
    }
    if !g.is_over() {
        // 止まる位置の影と、落ちているブロック
        for (x, y) in g.ghost() {
            if y >= 0 {
                c.round_rect(BOARD_X + x * CELL, BOARD_Y + y * CELL, CELL - 1, CELL - 1, &k, GHOST);
            }
        }
        let (cells, color) = g.piece();
        for (x, y) in cells {
            if y >= 0 {
                cell(c, &k, x, y, COLORS[color as usize - 1]);
            }
        }
    }
    // 右側: 次の形・点数・列・レベル
    let lh = text_height(FONT_UI);
    draw_text(c, FONT_UI, PANEL_X, BOARD_Y - 4, "NEXT", theme::TEXT_DIM);
    c.fill_round_rect(PANEL_X, BOARD_Y + 14, 40, 24, &panel, BOARD, 255);
    let next = g.next_kind();
    for (x, y) in blocks::shape_cells(next, 0) {
        let (px, py) = (PANEL_X + 4 + x * CELL, BOARD_Y + 18 + y * CELL);
        c.fill_round_rect(px, py, CELL - 1, CELL - 1, &k, COLORS[next as usize], 255);
    }
    let rows = [("SCORE", g.score()), ("LINES", g.lines()), ("LEVEL", g.level())];
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = BOARD_Y + 44 + i as i32 * 36;
        draw_text(c, FONT_UI, PANEL_X, y, label, theme::TEXT_DIM);
        draw_text(c, FONT_BOLD, PANEL_X, y + lh - 2, &format_fixed::<12>(format_args!("{value}")), theme::TEXT);
    }
    let banner = if g.is_over() {
        Some(("GAME OVER", "Enter: again"))
    } else if paused {
        Some(("PAUSED", "P: resume"))
    } else {
        None
    };
    if let Some((big, small)) = banner {
        let y = BOARD_Y + bh / 2 - 20;
        c.fill_round_rect(BOARD_X + 2, y, bw - 4, 40, &panel, theme::ACCENT, 240);
        draw_text_centered(c, FONT_BOLD, BOARD_X, bw, y + 2, big, rgb(255, 255, 255));
        draw_text_centered(c, FONT_UI, BOARD_X, bw, y + 20, small, theme::ACCENT_LIGHT);
    }
}
