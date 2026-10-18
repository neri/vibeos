//! 落ち物パズル（ウィンドウ）
//!
//! ← → で動かす、↑ で回す、↓ でゆっくり落とす、Space で一気に落とす、P で一時停止、
//! 終わったら Enter でもう一度、Esc か [x] で終わる。列が消えると音が鳴る。
//! 引数で乱数の種を指定できる（同じ種なら同じ順にブロックが出る。テスト用）。中身は `vibe_games::blocks`。

#![no_std]
#![no_main]

use vibe_games::blocks::{self, Blocks, Move};
use vibe_rt::abi::key;
use vibe_rt::gfx::{Event, Window, draw_text16_centered, font16};
use vibe_rt::gfx_lib::Surface;
use vibe_rt::gfx_lib::palette::{BLACK, DARK_GRAY, WHITE};
use vibe_rt::{format_fixed, println, sys};

vibe_rt::entry!(main);

const CELL: i32 = 8;
const BOARD_X: i32 = 8;
const BOARD_Y: i32 = 8;
const PANEL_X: i32 = BOARD_X + blocks::W as i32 * CELL + 10;
const W: usize = 152;
const H: usize = (BOARD_Y * 2 + blocks::H as i32 * CELL) as usize;

/// 盤面の地と、止まる位置の影（濃い灰の 1 ピクセルの枠は画面では点線）。白黒の画面（docs/plans/mono.md）なので、
/// 形（I O T S Z J L）は色ではなくマスの中の模様で見分ける（`block`）
const BOARD: u8 = WHITE;
const GHOST: u8 = DARK_GRAY;

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
        draw(&mut win.surface(), &game, paused);
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

/// 1 マス（7x7）: 黒い枠と、形 `kind`（0〜6 = I O T S Z J L）ごとの中（5x5）の模様:
/// ベタ・白抜き・市松・横縞・縦縞・真ん中の点・斜め
fn block(s: &mut Surface, px: i32, py: i32, kind: u8) {
    let n = CELL - 1;
    s.fill_rect(px, py, n, n, WHITE);
    s.rect(px, py, n, n, BLACK);
    for dy in 1..n - 1 {
        for dx in 1..n - 1 {
            let on = match kind {
                0 => true,
                1 => false,
                2 => (dx + dy) % 2 == 0,
                3 => dy % 2 == 1,
                4 => dx % 2 == 1,
                5 => (2..=4).contains(&dx) && (2..=4).contains(&dy),
                _ => (dx + dy) % 3 == 0,
            };
            if on {
                s.put_pixel(px + dx, py + dy, BLACK);
            }
        }
    }
}

fn cell(s: &mut Surface, x: i32, y: i32, kind: u8) {
    block(s, BOARD_X + x * CELL, BOARD_Y + y * CELL, kind);
}

fn draw(s: &mut Surface, g: &Blocks, paused: bool) {
    let f = font16();
    s.clear(WHITE);
    let (bw, bh) = (blocks::W as i32 * CELL, blocks::H as i32 * CELL);
    s.fill_rect(BOARD_X - 1, BOARD_Y - 1, bw + 2, bh + 2, BOARD);
    s.round_rect(BOARD_X - 3, BOARD_Y - 3, bw + 6, bh + 6, 0, 2, BLACK);
    for y in 0..blocks::H {
        for x in 0..blocks::W {
            let v = g.cell(x, y);
            if v != 0 {
                cell(s, x as i32, y as i32, v - 1);
            }
        }
    }
    if !g.is_over() {
        // 止まる位置の影と、落ちているブロック
        for (x, y) in g.ghost() {
            if y >= 0 {
                s.rect(BOARD_X + x * CELL, BOARD_Y + y * CELL, CELL - 1, CELL - 1, GHOST);
            }
        }
        let (cells, color) = g.piece();
        for (x, y) in cells {
            if y >= 0 {
                cell(s, x, y, color - 1);
            }
        }
    }
    // 右側: 次の形・点数・列・レベル
    s.draw_text(f, PANEL_X, BOARD_Y - 4, "NEXT", BLACK, None);
    s.fill_rect(PANEL_X, BOARD_Y + 14, 40, 24, BOARD);
    s.round_rect(PANEL_X - 2, BOARD_Y + 12, 44, 28, 0, 2, BLACK);
    let next = g.next_kind();
    for (x, y) in blocks::shape_cells(next, 0) {
        block(s, PANEL_X + 4 + x * CELL, BOARD_Y + 18 + y * CELL, next);
    }
    let rows = [("SCORE", g.score()), ("LINES", g.lines()), ("LEVEL", g.level())];
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = BOARD_Y + 44 + i as i32 * 36;
        s.draw_text(f, PANEL_X, y, label, BLACK, None);
        s.draw_text_bold(f, PANEL_X, y + f.height as i32, &format_fixed::<12>(format_args!("{value}")), BLACK);
    }
    let banner = if g.is_over() {
        Some(("GAME OVER", "[Enter]"))
    } else if paused {
        Some(("PAUSED", "P: resume"))
    } else {
        None
    };
    if let Some((big, small)) = banner {
        let y = BOARD_Y + bh / 2 - 20;
        s.fill_rect(BOARD_X, y, bw, 40, BLACK);
        draw_text16_centered(s, BOARD_X, bw, y + 3, big, WHITE, true);
        draw_text16_centered(s, BOARD_X, bw, y + 21, small, WHITE, false);
    }
}
