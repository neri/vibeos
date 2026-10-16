//! 電卓（ウィンドウ）。ボタンをマウスで押すか、キーボードで打つ
//!
//! キー: 数字・. + - * /、Enter か = で計算、BackSpace で 1 文字消す、Delete で入力中の数を消す、
//! Esc で全部消す。[x] で終わる。計算の中身は `vibe-calc`、部品は `vibe-ui`。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_calc::{Calc, Key, Op};
use vibe_rt::abi::key;
use vibe_rt::gfx::{Event, Window};
use vibe_rt::{println, sys};
use vibe_ui::{Action, Align, Button, Id, Input, Label, Rect, Ui};

vibe_rt::entry!(main);

const W: usize = 156;
const H: usize = 172;
const MARGIN: i32 = 8;
const DISPLAY_H: i32 = 24;
const GAP: i32 = 4;

/// ボタンの並び（左上から行ごと）
const BUTTONS: [(&str, Key); 20] = [
    ("C", Key::Clear),
    ("CE", Key::ClearEntry),
    ("<-", Key::Back),
    ("/", Key::Op(Op::Div)),
    ("7", Key::Digit(7)),
    ("8", Key::Digit(8)),
    ("9", Key::Digit(9)),
    ("*", Key::Op(Op::Mul)),
    ("4", Key::Digit(4)),
    ("5", Key::Digit(5)),
    ("6", Key::Digit(6)),
    ("-", Key::Op(Op::Sub)),
    ("1", Key::Digit(1)),
    ("2", Key::Digit(2)),
    ("3", Key::Digit(3)),
    ("+", Key::Op(Op::Add)),
    ("+/-", Key::Negate),
    ("0", Key::Digit(0)),
    (".", Key::Point),
    ("=", Key::Equals),
];

struct App {
    ui: Ui,
    display: Id,
    /// ボタンの部品の番号と、押したときのキー
    buttons: Vec<(Id, Key)>,
    calc: Calc,
}

fn main() -> i32 {
    vibe_ui::set_font(vibe_rt::gfx::font16());
    let Some(mut win) = Window::open(W, H, "Calc") else {
        println!("calc: cannot open a window");
        return 1;
    };
    let mut app = App::new();
    println!("calc: started");
    loop {
        app.draw(&mut win);
        let key = match win.event(None) {
            Some(Event::Close) | None => break,
            Some(Event::Key(k)) => key_of(k),
            Some(Event::Mouse { x, y, buttons }) => match app.ui.handle(Input::Mouse { x, y, buttons }, sys::ticks()) {
                Some(Action::Clicked(id)) => app.buttons.iter().find(|b| b.0 == id).map(|b| b.1),
                _ => None,
            },
        };
        if let Some(k) = key {
            app.calc.press(k);
            if k == Key::Equals {
                println!("calc: = {}", app.calc.display());
            }
        }
    }
    drop(win);
    println!("calc: closed");
    0
}

/// キーボードのキー → 電卓のキー（押したときだけ）
fn key_of(k: vibe_rt::abi::KeyEvent) -> Option<Key> {
    if !k.pressed() {
        return None;
    }
    match k.key() {
        key::ENTER => Some(Key::Equals),
        key::BACKSPACE => Some(Key::Back),
        key::DELETE => Some(Key::ClearEntry),
        key::ESCAPE => Some(Key::Clear),
        _ => k.ch().and_then(Key::from_char),
    }
}

impl App {
    fn new() -> App {
        let mut ui = Ui::new();
        let area = Rect::new(MARGIN, MARGIN, W as i32 - 2 * MARGIN, H as i32 - 2 * MARGIN);
        let (top, rest) = area.split_top(DISPLAY_H, GAP + 2);
        let display = ui.add(top, Label::new("0").align(Align::Right).display());
        let buttons = rest
            .grid(4, 5, GAP)
            .into_iter()
            .zip(BUTTONS)
            .map(|(r, (text, key))| (ui.add(r, Button::new(text)), key))
            .collect();
        App { ui, display, buttons, calc: Calc::new() }
    }

    fn draw(&mut self, win: &mut Window) {
        let text = self.calc.display();
        self.ui.set_text(self.display, &text);
        let mut s = win.surface();
        s.clear(vibe_ui::draw::FACE);
        self.ui.draw(&mut s);
        win.present();
    }
}
