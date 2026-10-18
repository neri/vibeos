//! ヘビのゲーム（320x200 のウィンドウ）: 矢印キーで向きを変え、えさを食べると伸びる。壁か自分に当たると終わり。
//! Esc か [x] で終わる。終わったら Enter でもう一度。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;

use vibe_rt::gfx::{Event, HEIGHT, WIDTH, Window, font16};
use vibe_rt::gfx_lib::palette;
use vibe_rt::{Rng, abi, println, sys};

vibe_rt::entry!(main);

const CELL: i32 = 10;
const COLS: i32 = 32;
const ROWS: i32 = 18;
const TOP: i32 = 20;
/// 1 歩の間隔（タイマの刻み。100 で 1 秒）
const STEP_TICKS: u32 = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
struct P(i32, i32);

struct Game {
    body: VecDeque<P>,
    dir: P,
    next_dir: P,
    food: P,
    score: u32,
    over: bool,
}

impl Game {
    fn new(rng: &mut Rng) -> Game {
        let mut body = VecDeque::new();
        for i in 0..4 {
            body.push_back(P(10 - i, ROWS / 2));
        }
        let mut g = Game { body, dir: P(1, 0), next_dir: P(1, 0), food: P(0, 0), score: 0, over: false };
        g.place_food(rng);
        g
    }

    fn place_food(&mut self, rng: &mut Rng) {
        loop {
            let p = P(rng.below(COLS as u32) as i32, rng.below(ROWS as u32) as i32);
            if !self.body.contains(&p) {
                self.food = p;
                return;
            }
        }
    }

    fn turn(&mut self, d: P) {
        // 真後ろには曲がれない
        if d.0 != -self.dir.0 || d.1 != -self.dir.1 {
            self.next_dir = d;
        }
    }

    fn step(&mut self, rng: &mut Rng) {
        self.dir = self.next_dir;
        let head = self.body[0];
        let next = P(head.0 + self.dir.0, head.1 + self.dir.1);
        let grows = next == self.food;
        // 尻尾は動くので、伸びないときは当たり判定から外す
        let tail_moves = !grows;
        let hits_self = self.body.iter().take(self.body.len() - tail_moves as usize).any(|&b| b == next);
        if next.0 < 0 || next.0 >= COLS || next.1 < 0 || next.1 >= ROWS || hits_self {
            self.over = true;
            sys::sound(110, 400);
            return;
        }
        self.body.push_front(next);
        if grows {
            self.score += 10;
            sys::sound(880, 60);
            self.place_food(rng);
        } else {
            self.body.pop_back();
        }
    }

    fn draw(&self, win: &mut Window) {
        let f = font16();
        let mut s = win.surface();
        // 白黒の画面（docs/plans/mono.md）: 白い地、上に黒い帯。頭は黒、体は市松（濃い灰）、えさは白抜きの丸
        s.clear(palette::WHITE);
        s.fill_rect(0, 0, WIDTH as i32, TOP - 2, palette::BLACK);
        s.draw_text_bold(f, 4, 1, "SNAKE", palette::WHITE);
        let score = format!("SCORE {}", self.score);
        s.draw_text(f, WIDTH as i32 - 4 - score.len() as i32 * 8, 1, &score, palette::WHITE, None);
        let cell = |s: &mut vibe_rt::gfx_lib::Surface, p: P, c: u8| {
            s.fill_rect(p.0 * CELL + 1, TOP + p.1 * CELL + 1, CELL - 2, CELL - 2, c);
        };
        let (fx, fy) = (self.food.0 * CELL + CELL / 2, TOP + self.food.1 * CELL + CELL / 2);
        s.circle(fx, fy, CELL / 2 - 1, palette::BLACK);
        s.circle(fx, fy, CELL / 2 - 2, palette::BLACK);
        for (i, &b) in self.body.iter().enumerate() {
            cell(&mut s, b, if i == 0 { palette::BLACK } else { palette::DARK_GRAY });
        }
        if self.over {
            let msg = "GAME OVER - ENTER: again  ESC: quit";
            let x = (WIDTH as i32 - msg.len() as i32 * 8) / 2;
            s.fill_rect(x - 4, 92, msg.len() as i32 * 8 + 8, 20, palette::BLACK);
            s.draw_text(f, x, 94, msg, palette::WHITE, None);
        }
    }
}

fn main() -> i32 {
    let Some(mut win) = Window::open(WIDTH, HEIGHT, "Snake") else {
        println!("snake: cannot open a window");
        return 1;
    };
    let mut rng = Rng::new(sys::ticks() ^ 0x5EED);
    let mut game = Game::new(&mut rng);
    let mut best = 0;
    let mut last_step = sys::ticks();
    game.draw(&mut win);
    win.present();
    println!("snake: started");
    loop {
        // 次の 1 歩まで（終わっていればいつまでも）キーを待つ
        let wait = (!game.over).then(|| STEP_TICKS.saturating_sub(sys::ticks().wrapping_sub(last_step)) * 10);
        match win.event(wait) {
            Some(Event::Close) => break,
            Some(Event::Key(k)) if k.pressed() => match k.key() {
                abi::key::ESCAPE => break,
                abi::key::UP => game.turn(P(0, -1)),
                abi::key::DOWN => game.turn(P(0, 1)),
                abi::key::LEFT => game.turn(P(-1, 0)),
                abi::key::RIGHT => game.turn(P(1, 0)),
                abi::key::ENTER if game.over => {
                    game = Game::new(&mut rng);
                    last_step = sys::ticks();
                    game.draw(&mut win);
                    win.present();
                }
                _ => {}
            },
            _ => {}
        }
        let now = sys::ticks();
        if !game.over && now.wrapping_sub(last_step) >= STEP_TICKS {
            last_step = now;
            game.step(&mut rng);
            best = best.max(game.score);
            game.draw(&mut win);
            win.present();
            if game.over {
                println!("snake: game over, score {}", game.score);
            }
        }
    }
    drop(win);
    println!("snake: best score {best}");
    0
}
