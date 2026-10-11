//! 入力のまとめ: マウスのパケットを解釈してカーソルを動かすスレッドと、アプリ向けの API
//!
//! マウスの状態（位置・ボタン）はこのスレッドが更新し、画面のカーソルも動かす。アプリは
//! `poll_mouse` でイベントを受け取る。

use vibe_keyboard::mouse::{MouseDecoder, MousePacket};

use crate::drivers::vga::{self, SCREEN};
use crate::drivers::mouse;
use crate::sync::IrqMutex;
use crate::task::{self, wait::WaitQueue};
use crate::util::ring::Ring;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseEvent {
    /// 動いた後の位置（画面の中に収める）
    pub x: i32,
    pub y: i32,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

static STATE: IrqMutex<MouseEvent> = IrqMutex::new(MouseEvent { x: 0, y: 0, left: false, right: false, middle: false });
static EVENTS: IrqMutex<Ring<MouseEvent, 64>> = IrqMutex::new(Ring::new(MouseEvent {
    x: 0,
    y: 0,
    left: false,
    right: false,
    middle: false,
}));
/// キーボードかマウスに何か来たら起こされる
static INPUT_WAIT: WaitQueue = WaitQueue::new();

/// キーボード・マウスの割り込みやスレッドから呼ぶ
pub fn notify() {
    INPUT_WAIT.notify_all();
}

pub fn start_mouse() {
    {
        let mut s = STATE.lock();
        s.x = (vga::WIDTH / 2) as i32;
        s.y = (vga::HEIGHT / 2) as i32;
    }
    task::spawn("mouse", || {
        let mut decoder = MouseDecoder::new();
        loop {
            mouse::RAW_WAIT.wait_until(mouse::has_raw);
            while let Some(b) = mouse::pop_raw() {
                if let Some(p) = decoder.feed(b) {
                    apply(p);
                }
            }
        }
    });
}

fn apply(p: MousePacket) {
    let ev = {
        let mut s = STATE.lock();
        s.x = (s.x + p.dx as i32).clamp(0, vga::WIDTH as i32 - 1);
        s.y = (s.y + p.dy as i32).clamp(0, vga::HEIGHT as i32 - 1);
        s.left = p.left;
        s.right = p.right;
        s.middle = p.middle;
        *s
    };
    {
        let mut screen = SCREEN.lock();
        screen.set_cursor(ev.x, ev.y);
        screen.present();
    }
    let mut q = EVENTS.lock();
    if !q.push(ev) {
        // あふれたら古いものを捨てて新しいものを入れる
        q.pop();
        q.push(ev);
    }
    drop(q);
    notify();
}

/// 今のマウスの状態
#[allow(dead_code)]
pub fn mouse_state() -> MouseEvent {
    *STATE.lock()
}

/// たまっているマウスのイベントを 1 つ取り出す（待たない）
pub fn poll_mouse() -> Option<MouseEvent> {
    EVENTS.lock().pop()
}

/// 前回までのマウスのイベントを捨てる（アプリを始めるときに）
pub fn clear_mouse_events() {
    while poll_mouse().is_some() {}
}
