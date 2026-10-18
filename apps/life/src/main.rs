//! ライフゲーム。クリックでセルを反転、スペースで動かす/止める、R で乱数、C で消す。[x] か Esc で終わる

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use vibe_rt::abi::key;
use vibe_rt::gfx::{Event, Window};
use vibe_rt::gfx_lib::palette;
use vibe_rt::{Rng, println, sys};

vibe_rt::entry!(main);

const CELL: usize = 4;
const COLS: usize = 48;
const ROWS: usize = 36;

struct Life {
    cells: Vec<bool>,
}

impl Life {
    fn get(&self, x: isize, y: isize) -> bool {
        // 端はつながっている（トーラス）
        let x = x.rem_euclid(COLS as isize) as usize;
        let y = y.rem_euclid(ROWS as isize) as usize;
        self.cells[y * COLS + x]
    }

    fn step(&mut self) {
        let mut next = vec![false; COLS * ROWS];
        for y in 0..ROWS as isize {
            for x in 0..COLS as isize {
                let n = (-1..=1)
                    .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                    .filter(|&(dx, dy)| (dx, dy) != (0, 0) && self.get(x + dx, y + dy))
                    .count();
                next[y as usize * COLS + x as usize] = matches!((self.get(x, y), n), (true, 2) | (_, 3));
            }
        }
        self.cells = next;
    }
}

fn main() -> i32 {
    let Some(mut win) = Window::open(COLS * CELL, ROWS * CELL, "Life") else {
        println!("life: cannot open a window");
        return 1;
    };
    let mut life = Life { cells: vec![false; COLS * ROWS] };
    // グライダーを 1 つ置いておく
    for (x, y) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
        life.cells[y * COLS + x] = true;
    }
    let mut running = false;
    let mut rng = Rng::new(sys::ticks() | 1);
    let mut generation = 0u32;
    let mut last_toggle: Option<(usize, usize)> = None;
    println!("life: started");
    loop {
        draw(&mut win, &life);
        win.present();
        let ev = win.event(Some(if running { 100 } else { 1000 }));
        match ev {
            Some(Event::Close) => break,
            Some(Event::Key(k)) if k.pressed() => match (k.key(), k.ch()) {
                (key::ESCAPE, _) => break,
                (_, Some(' ')) => {
                    running = !running;
                    println!("life: {}", if running { "running" } else { "paused" });
                }
                (_, Some('r' | 'R')) => life.cells.iter_mut().for_each(|c| *c = rng.below(4) == 0),
                (_, Some('c' | 'C')) => life.cells.iter_mut().for_each(|c| *c = false),
                (_, Some('s' | 'S')) => {
                    life.step();
                    generation += 1;
                }
                _ => {}
            },
            Some(Event::Mouse { x, y, buttons }) => {
                if buttons & 1 != 0 {
                    let cell = (x as usize / CELL, y as usize / CELL);
                    // ドラッグ中に同じセルを何度も反転しない
                    if last_toggle != Some(cell) && cell.0 < COLS && cell.1 < ROWS {
                        let i = cell.1 * COLS + cell.0;
                        life.cells[i] = !life.cells[i];
                        last_toggle = Some(cell);
                    }
                } else {
                    last_toggle = None;
                }
            }
            None if running => {
                life.step();
                generation += 1;
            }
            _ => {}
        }
    }
    drop(win);
    println!("life: closed after {generation} generations");
    0
}

fn draw(win: &mut Window, life: &Life) {
    let mut s = win.surface();
    // 白黒の画面（docs/plans/mono.md）: 白い地に黒いセル
    s.clear(palette::WHITE);
    for y in 0..ROWS {
        for x in 0..COLS {
            if life.cells[y * COLS + x] {
                s.fill_rect((x * CELL) as i32, (y * CELL) as i32, CELL as i32 - 1, CELL as i32 - 1, palette::BLACK);
            }
        }
    }
}
