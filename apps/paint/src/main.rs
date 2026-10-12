//! お絵かき: マウスの左ボタンで線を描く。上の色の並びをクリックで色を選ぶ。
//! C で全部消す、Esc で終わる。
//!
//! デスクトップでは好きな大きさのキャンバスのウィンドウ（`paint 幅 高さ`、既定は 320x200。[x] でも終わる）、
//! 320x200 では全画面。

#![no_std]
#![no_main]

use vibe_rt::gfx::{Event, HEIGHT, Screen, WIDTH, Window};
use vibe_rt::gfx_lib::{Surface, palette};
use vibe_rt::{abi, println, sys};

vibe_rt::entry!(main);

const TOOLBAR_H: i32 = 12;
const CANVAS_BG: u8 = palette::WHITE;
/// ウィンドウのキャンバスの大きさの範囲
const MIN_W: usize = 160;
const MIN_H: usize = 40;

fn main() -> i32 {
    if vibe_rt::gfx::is_desktop() {
        let mut args = vibe_rt::args().skip(1).map(|a| a.parse::<usize>().ok());
        let w = args.next().flatten().unwrap_or(WIDTH);
        let h = args.next().flatten().unwrap_or(HEIGHT);
        return run_window(w, h);
    }
    run_fullscreen()
}

/// 描いている状態（色と、前のマウスの位置）
struct Paint {
    color: u8,
    last: Option<(i32, i32)>,
}

impl Paint {
    /// 色の並びの 1 つの幅（16 色を横に並べる）
    fn swatch(width: i32) -> i32 {
        (width / 16).max(1)
    }

    fn clear(&self, s: &mut Surface) {
        let (w, h) = (s.width() as i32, s.height() as i32);
        s.fill_rect(0, 0, w, h, CANVAS_BG);
        self.toolbar(s);
    }

    fn toolbar(&self, s: &mut Surface) {
        let w = s.width() as i32;
        let sw = Self::swatch(w);
        s.fill_rect(0, 0, w, TOOLBAR_H, palette::DARK_GRAY);
        for i in 0..16 {
            s.fill_rect(i * sw + 1, 1, sw - 2, TOOLBAR_H - 2, i as u8);
            if i as u8 == self.color {
                s.rect(i * sw, 0, sw, TOOLBAR_H, palette::YELLOW);
            }
        }
    }

    /// マウスの状態を 1 つ処理する。描き直したら true
    fn mouse(&mut self, s: &mut Surface, x: i32, y: i32, buttons: u32) -> bool {
        if buttons & 1 == 0 {
            self.last = None;
            return false;
        }
        if y < TOOLBAR_H {
            let i = x / Self::swatch(s.width() as i32);
            self.last = None;
            if (0..16).contains(&i) {
                self.color = i as u8;
                self.toolbar(s);
                return true;
            }
            return false;
        }
        let (x0, y0) = self.last.unwrap_or((x, y));
        s.line(x0, y0.max(TOOLBAR_H), x, y, self.color);
        self.last = Some((x, y));
        true
    }

    /// キーを 1 つ処理する。終わるなら Err、描き直したら Ok(true)
    fn key(&mut self, s: &mut Surface, k: abi::KeyEvent) -> Result<bool, ()> {
        if !k.pressed() {
            return Ok(false);
        }
        match (k.key(), k.ch()) {
            (abi::key::ESCAPE, _) => Err(()),
            (_, Some('c' | 'C')) => {
                self.clear(s);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

fn run_window(w: usize, h: usize) -> i32 {
    let info = sys::sys_info();
    let w = w.clamp(MIN_W, info.screen_width as usize - 8);
    let h = h.clamp(MIN_H, info.screen_height as usize - 32 - TOOLBAR_H as usize);
    let Some(mut win) = Window::open(w, h + TOOLBAR_H as usize, "Paint") else {
        println!("paint: cannot open a window");
        return 1;
    };
    let mut p = Paint { color: palette::BLACK, last: None };
    p.clear(&mut win.surface());
    win.present();
    println!("paint: started ({w}x{h})");
    loop {
        let changed = match win.event(None) {
            Some(Event::Close) | None => break,
            Some(Event::Key(k)) => match p.key(&mut win.surface(), k) {
                Err(()) => break,
                Ok(c) => c,
            },
            Some(Event::Mouse { x, y, buttons }) => p.mouse(&mut win.surface(), x, y, buttons),
        };
        if changed {
            win.present();
        }
    }
    drop(win);
    println!("paint: closed");
    0
}

fn run_fullscreen() -> i32 {
    let Some(mut screen) = Screen::open(true) else {
        println!("paint: cannot open the screen");
        return 1;
    };
    let mut p = Paint { color: palette::BLACK, last: None };
    p.clear(&mut screen.surface());
    screen.present();
    println!("paint: started");
    'main: loop {
        let mut changed = false;
        while let Some(k) = sys::getkey(true) {
            match p.key(&mut screen.surface(), k) {
                Err(()) => break 'main,
                Ok(c) => changed |= c,
            }
        }
        while let Some(m) = sys::mouse() {
            changed |= p.mouse(&mut screen.surface(), m.x, m.y, m.buttons);
        }
        if changed {
            screen.present();
        } else {
            sys::sleep(10);
        }
    }
    drop(screen);
    println!("paint: closed");
    0
}
