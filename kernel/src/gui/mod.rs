//! ウィンドウの重ね合わせ（コンポジタ）と入力の振り分け
//!
//! 2 つのモードがある:
//! - **全画面**（320x200）: 前面のウィンドウ 1 つだけを画面いっぱいに出す（これまでの動き）
//! - **デスクトップ**（640x480）: 上端のバー、背景、重なったウィンドウ（タイトルバーと枠付き）を出す。
//!   クリックで前に出す（フォーカス）、タイトルバーのドラッグで動かす、[x] で閉じる
//!
//! 0 番のウィンドウはコンソール（シェルの端末）で、バッファは静的な領域（起動直後から使うため）。
//! 描画は各ウィンドウのバッファに行い、`present` で書き換えた行だけを重ね合わせて VRAM に送る。
//! マウスカーソルは重ね合わせのときに上に描く。
//!
//! パレット: 全画面では前面のウィンドウの独自のパレットを VGA にそのまま設定する。デスクトップでは
//! 他のウィンドウの色が変わらないよう VGA は標準のパレットのまま、独自のパレットを持つウィンドウの
//! 中身は重ね合わせるときに標準のパレットの近い色に置き換える。
//!
//! 入力: キーはフォーカスのあるウィンドウ（全画面では前面）に、マウスはその中の座標にして届ける。
//! ウィンドウの持ち主はそれぞれのキューから読む（`read_key` / `poll_mouse`）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_font::FONT_8X8;
use vibe_gfx::{Surface, palette};
use vibe_keyboard::{KeyCode, KeyEvent, Modifiers};

use crate::drivers::vga::{self, MAX_HEIGHT, MAX_WIDTH};
use crate::sync::{BootCell, IrqMutex};
use crate::task::sched::TaskId;
use crate::task::wait::WaitQueue;
use crate::util::ring::Ring;

pub type WinId = u32;
pub const CONSOLE: WinId = 0;

/// タイトルバーの高さ・デスクトップの上端のバーの高さ
pub const TITLE_H: usize = 12;
pub const BAR_H: usize = 10;
const CLOSE_W: usize = 9;
const DESKTOP_BG: u8 = palette::CUBE_BASE + 13; // 落ち着いた青緑（カラーキューブの 0,2,1）
const BORDER: u8 = palette::LIGHT_GRAY;
const TITLE_ACTIVE: u8 = palette::BLUE;
const TITLE_INACTIVE: u8 = palette::DARK_GRAY;
/// 端末のウィンドウの大きさ（文字数）
const TERMINAL_COLS: usize = 72;
const TERMINAL_ROWS: usize = 46;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Fullscreen,
    Desktop,
}

/// ウィンドウの中の座標でのマウスの状態
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseEvent {
    pub x: i32,
    pub y: i32,
    /// bit0 = 左, bit1 = 右, bit2 = 中
    pub buttons: u8,
}

const NO_KEY: KeyEvent = KeyEvent {
    key: KeyCode(0),
    pressed: false,
    mods: Modifiers {
        left_shift: false,
        right_shift: false,
        left_ctrl: false,
        right_ctrl: false,
        left_alt: false,
        right_alt: false,
        caps_lock: false,
    },
    ch: None,
};

struct Window {
    w: usize,
    h: usize,
    /// None ならコンソール（静的な領域）
    buf: Option<Box<[u8]>>,
    /// 中身の左上の画面上の位置（デスクトップのとき）
    x: i32,
    y: i32,
    title: String,
    owner: Option<TaskId>,
    /// [x] で持ち主のプロセスを止める（gfx_open の画面）
    kill_on_close: bool,
    closable: bool,
    titlebar: Vec<u8>,
    /// 独自のパレット（全画面で前に出ている間だけ使う）
    palette: Option<Box<[[u8; 3]; 256]>>,
    /// 独自のパレットの色 → 標準のパレットの近い色（デスクトップで重ね合わせるときに使う）
    remap: Option<Box<[u8; 256]>>,
    /// アプリが忙しい間に打たれたキーも落とさないよう、押す・離すで 128 打鍵分
    keys: Ring<KeyEvent, 256>,
    mouse: Ring<MouseEvent, 64>,
    close_requested: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Title,
    Close,
    Client,
}

static CONSOLE_BUF: BootCell<[u8; MAX_WIDTH * MAX_HEIGHT]> = BootCell::new([0; MAX_WIDTH * MAX_HEIGHT]);
static BAR_BUF: BootCell<[u8; MAX_WIDTH * BAR_H]> = BootCell::new([0; MAX_WIDTH * BAR_H]);

pub struct Screen {
    mode: Mode,
    width: usize,
    height: usize,
    console_w: usize,
    console_h: usize,
    windows: BTreeMap<WinId, Window>,
    /// 重なりの順（後ろが手前）
    order: Vec<WinId>,
    next_id: WinId,
    dirty_lo: usize,
    dirty_hi: usize,
    cursor_x: i32,
    cursor_y: i32,
    cursor_visible: bool,
    /// タイトルバーをドラッグ中: (ウィンドウ, つかんだ位置の中身の左上からのずれ)
    drag: Option<(WinId, i32, i32)>,
    /// 中身でボタンを押したウィンドウ（離すまでマウスを届ける）
    capture: Option<WinId>,
    buttons: u8,
    placed: u32,
}

/// 画面。コンソール（gfx::console の CONSOLE）と両方取るときはコンソールを先に取ること
pub static SCREEN: IrqMutex<Screen> = IrqMutex::new(Screen {
    mode: Mode::Fullscreen,
    width: 0,
    height: 0,
    console_w: 0,
    console_h: 0,
    windows: BTreeMap::new(),
    order: Vec::new(),
    next_id: 0,
    dirty_lo: 0,
    dirty_hi: 0,
    cursor_x: 0,
    cursor_y: 0,
    cursor_visible: false,
    drag: None,
    capture: None,
    buttons: 0,
    placed: 0,
});

/// キー・マウス・閉じる要求が届いたら起こされる
static INPUT_WAIT: WaitQueue = WaitQueue::new();

/// 起動直後（vga::init の後）に呼ぶ。まずはコンソールを画面いっぱいに出す
pub fn init() {
    let mut s = SCREEN.lock();
    s.width = vga::width();
    s.height = vga::height();
    s.console_w = s.width;
    s.console_h = s.height;
    s.cursor_x = (s.width / 2) as i32;
    s.cursor_y = (s.height / 2) as i32;
    vga::set_palette(&palette::standard());
    s.surface().clear(palette::BLACK);
    let h = s.height;
    s.mark_rows(0, h);
    s.present();
}

/// ヒープが使えるようになったら呼ぶ。コンソールをウィンドウとして登録する
pub fn init_windows() {
    let mut s = SCREEN.lock();
    let (w, h) = (s.console_w, s.console_h);
    let win = new_window(w, h, None, "Terminal", None, false, false);
    s.windows.insert(CONSOLE, win);
    s.order.push(CONSOLE);
}

/// 画面が広ければデスクトップにする（スケジューラが動いた後に呼ぶ）。したら true
pub fn start_desktop() -> bool {
    let mut s = SCREEN.lock();
    if s.width < 640 || s.height < 400 {
        return false;
    }
    s.mode = Mode::Desktop;
    let (cw, ch) = (TERMINAL_COLS * FONT_8X8.width, TERMINAL_ROWS * FONT_8X8.height);
    s.console_w = cw.min(s.width - 2);
    s.console_h = ch.min(s.height - BAR_H - TITLE_H - 2);
    let (cw, ch) = (s.console_w, s.console_h);
    let y = (BAR_H + TITLE_H + 1 + 8) as i32;
    if let Some(c) = s.windows.get_mut(&CONSOLE) {
        c.w = cw;
        c.h = ch;
        c.x = 16;
        c.y = y;
        c.closable = false;
    }
    s.surface().clear(palette::BLACK);
    s.refresh_titlebars();
    s.cursor_visible = true;
    // 上端のバーを塗っておく
    let w = s.width;
    // SAFETY: BAR_BUF は SCREEN のロックの中でだけ触る
    let bar = unsafe { &mut (&mut *BAR_BUF.as_ptr())[..w * BAR_H] };
    bar.fill(palette::BLUE);
    let h = s.height;
    s.mark_rows(0, h);
    s.present();
    true
}

#[allow(dead_code)]
pub fn mode() -> Mode {
    SCREEN.lock().mode
}

fn new_window(
    w: usize,
    h: usize,
    buf: Option<Box<[u8]>>,
    title: &str,
    owner: Option<TaskId>,
    kill_on_close: bool,
    closable: bool,
) -> Window {
    Window {
        w,
        h,
        buf,
        x: 0,
        y: 0,
        title: String::from(title),
        owner,
        kill_on_close,
        closable,
        titlebar: Vec::new(),
        palette: None,
        remap: None,
        keys: Ring::new(NO_KEY),
        mouse: Ring::new(MouseEvent { x: 0, y: 0, buttons: 0 }),
        close_requested: false,
    }
}

impl Screen {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// コンソールの画面（端末の中身）
    pub fn surface(&mut self) -> Surface<'_> {
        let (w, h) = (self.console_w, self.console_h);
        // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る
        let buf = unsafe { &mut (&mut *CONSOLE_BUF.as_ptr())[..w * h] };
        Surface::new(buf, w, h)
    }

    /// コンソールの大きさ（ピクセル）
    pub fn console_size(&self) -> (usize, usize) {
        (self.console_w, self.console_h)
    }

    /// コンソールの行 [y0, y1) を書き換えたことを記録する
    pub fn mark_dirty(&mut self, y0: usize, y1: usize) {
        self.mark_window_dirty(CONSOLE, y0, y1);
    }

    /// 上端のバー（デスクトップ）またはコンソールの 1 行目（全画面）に描く
    pub fn draw_status_bar(&mut self, draw: impl FnOnce(&mut Surface)) {
        match self.mode {
            Mode::Desktop => {
                let w = self.width;
                // SAFETY: BAR_BUF は SCREEN のロックの中でだけ触る
                let bar = unsafe { &mut (&mut *BAR_BUF.as_ptr())[..w * BAR_H] };
                draw(&mut Surface::new(bar, w, BAR_H));
                self.mark_rows(0, BAR_H);
            }
            Mode::Fullscreen => {
                draw(&mut self.surface());
                self.mark_dirty(0, FONT_8X8.height);
            }
        }
    }

    /// ウィンドウを作って前に出す
    pub fn create_window(&mut self, w: usize, h: usize, title: &str, owner: Option<TaskId>, kill_on_close: bool) -> WinId {
        self.next_id += 1; // 0 はコンソールなので 1 から
        let id = self.next_id;
        let buf = vec![palette::BLACK; w * h].into_boxed_slice();
        let mut win = new_window(w, h, Some(buf), title, owner, kill_on_close, true);
        if self.mode == Mode::Desktop {
            // 少しずつずらして置く
            let n = (self.placed % 8) as i32;
            self.placed += 1;
            let max_x = (self.width as i32 - w as i32 - 1).max(1);
            win.x = (120 + n * 24).min(max_x);
            win.y = ((BAR_H + TITLE_H + 1) as i32 + 24 + n * 20).min((self.height as i32 - h as i32 - 1).max(30));
        }
        self.windows.insert(id, win);
        self.order.push(id);
        self.focus_changed();
        id
    }

    pub fn destroy_window(&mut self, id: WinId) {
        if id == CONSOLE || self.windows.remove(&id).is_none() {
            return;
        }
        self.order.retain(|&w| w != id);
        if self.drag.is_some_and(|d| d.0 == id) {
            self.drag = None;
        }
        if self.capture == Some(id) {
            self.capture = None;
        }
        self.focus_changed();
    }

    pub fn window_surface(&mut self, id: WinId) -> Option<Surface<'_>> {
        if id == CONSOLE {
            return Some(self.surface());
        }
        let win = self.windows.get_mut(&id)?;
        let (w, h) = (win.w, win.h);
        win.buf.as_mut().map(|b| Surface::new(b, w, h))
    }

    /// ウィンドウ `id` の中身の行 [y0, y1) を書き換えたことを記録する
    pub fn mark_window_dirty(&mut self, id: WinId, y0: usize, y1: usize) {
        match self.mode {
            Mode::Fullscreen => {
                if self.foreground() == id {
                    self.mark_rows(y0, y1);
                }
            }
            Mode::Desktop => {
                let Some(win) = self.windows.get(&id) else {
                    // まだ登録していないコンソール（起動直後）
                    if id == CONSOLE {
                        self.mark_rows(y0, y1);
                    }
                    return;
                };
                let top = win.y + y0 as i32;
                let bottom = win.y + y1 as i32;
                if bottom > 0 {
                    self.mark_rows(top.max(0) as usize, bottom as usize);
                }
            }
        }
    }

    /// ウィンドウの独自のパレットを設定する（前に出ている間だけ使われる）
    pub fn set_window_palette(&mut self, id: WinId, start: usize, colors: &[[u8; 3]]) {
        let Some(win) = self.windows.get_mut(&id) else { return };
        let pal = win.palette.get_or_insert_with(|| Box::new(palette::standard()));
        for (i, c) in colors.iter().enumerate() {
            if let Some(slot) = pal.get_mut(start + i) {
                *slot = [c[0] & 0x3F, c[1] & 0x3F, c[2] & 0x3F];
            }
        }
        let std = palette::standard();
        let remap = win.remap.get_or_insert_with(|| Box::new(core::array::from_fn(|i| i as u8)));
        for i in start..(start + colors.len()).min(256) {
            let [r, g, b] = pal[i];
            remap[i] = palette::nearest(&std, r << 2, g << 2, b << 2);
        }
        let h = self.height;
        self.mark_rows(0, h);
        if self.focused() == Some(id) {
            self.apply_palette();
        }
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        // デスクトップではいつも出す
        let visible = visible || self.mode == Mode::Desktop;
        if self.cursor_visible != visible {
            self.cursor_visible = visible;
            self.mark_cursor_rows();
        }
    }

    fn set_cursor(&mut self, x: i32, y: i32) {
        if (x, y) == (self.cursor_x, self.cursor_y) {
            return;
        }
        self.mark_cursor_rows();
        self.cursor_x = x;
        self.cursor_y = y;
        self.mark_cursor_rows();
    }

    fn mark_cursor_rows(&mut self) {
        let y = self.cursor_y.max(0) as usize;
        self.mark_rows(y, y + CURSOR_SHAPE.len());
    }

    fn mark_rows(&mut self, y0: usize, y1: usize) {
        let y1 = y1.min(self.height);
        if y0 >= y1 {
            return;
        }
        if self.dirty_lo >= self.dirty_hi {
            self.dirty_lo = y0;
            self.dirty_hi = y1;
        } else {
            self.dirty_lo = self.dirty_lo.min(y0);
            self.dirty_hi = self.dirty_hi.max(y1);
        }
    }

    /// 全画面のときの前面のウィンドウ
    fn foreground(&self) -> WinId {
        self.order.last().copied().unwrap_or(CONSOLE)
    }

    /// キーを受け取るウィンドウ
    fn focused(&self) -> Option<WinId> {
        self.order.last().copied()
    }

    /// 前に出す
    fn raise(&mut self, id: WinId) {
        if self.focused() == Some(id) || !self.windows.contains_key(&id) {
            return;
        }
        self.order.retain(|&w| w != id);
        self.order.push(id);
        self.focus_changed();
    }

    /// 重なりの順やフォーカスが変わった: タイトルバー・パレットを直して全体を描き直す
    fn focus_changed(&mut self) {
        self.refresh_titlebars();
        self.apply_palette();
        let h = self.height;
        self.mark_rows(0, h);
    }

    fn apply_palette(&self) {
        if self.mode == Mode::Desktop {
            return; // デスクトップでは標準のパレットのまま（重ね合わせるときに置き換える）
        }
        match self.focused().and_then(|id| self.windows.get(&id)).and_then(|w| w.palette.as_deref()) {
            Some(p) => vga::set_palette(p),
            None => vga::set_palette(&palette::standard()),
        }
    }

    fn refresh_titlebars(&mut self) {
        if self.mode != Mode::Desktop {
            return;
        }
        let focused = self.focused();
        for (&id, win) in self.windows.iter_mut() {
            render_titlebar(win, Some(id) == focused);
        }
    }

    /// (x, y) にある一番手前のウィンドウとその部分
    fn hit(&self, x: i32, y: i32) -> Option<(WinId, Part)> {
        for &id in self.order.iter().rev() {
            let w = &self.windows[&id];
            let (left, right) = (w.x - 1, w.x + w.w as i32);
            let (top, bottom) = (w.y - TITLE_H as i32 - 1, w.y + w.h as i32);
            if x < left || x > right || y < top || y > bottom {
                continue;
            }
            if y < w.y {
                let close_x = w.x + w.w as i32 - CLOSE_W as i32 - 2;
                if w.closable && x >= close_x && x < close_x + CLOSE_W as i32 {
                    return Some((id, Part::Close));
                }
                return Some((id, Part::Title));
            }
            return Some((id, Part::Client));
        }
        None
    }

    fn move_window(&mut self, id: WinId, x: i32, y: i32) {
        let (sw, sh) = (self.width as i32, self.height as i32);
        let Some(win) = self.windows.get_mut(&id) else { return };
        let old = (win.y - TITLE_H as i32 - 1, win.y + win.h as i32 + 1);
        // タイトルバーが上端のバーの下に、少なくとも一部が画面に残るようにする
        win.x = x.clamp(-(win.w as i32) + 40, sw - 40);
        win.y = y.clamp((BAR_H + TITLE_H + 1) as i32, sh - 4);
        let new = (win.y - TITLE_H as i32 - 1, win.y + win.h as i32 + 1);
        for (a, b) in [old, new] {
            self.mark_rows(a.max(0) as usize, b.max(0) as usize);
        }
    }

    /// 入力スレッドから: マウスが (x, y)（画面の座標）にあり、ボタンが `buttons`
    fn on_mouse(&mut self, x: i32, y: i32, buttons: u8) -> Option<TaskId> {
        self.set_cursor(x, y);
        let left = buttons & 1 != 0;
        let was_left = self.buttons & 1 != 0;
        self.buttons = buttons;
        let mut kill = None;

        if self.mode == Mode::Fullscreen {
            let fg = self.foreground();
            if let Some(w) = self.windows.get_mut(&fg) {
                let ev = MouseEvent { x: x.clamp(0, w.w as i32 - 1), y: y.clamp(0, w.h as i32 - 1), buttons };
                push_mouse(w, ev);
            }
            return None;
        }

        if let Some((id, dx, dy)) = self.drag {
            if left {
                self.move_window(id, x - dx, y - dy);
                return None;
            }
            self.drag = None;
        } else if left && !was_left {
            match self.hit(x, y) {
                Some((id, Part::Title)) => {
                    self.raise(id);
                    let w = &self.windows[&id];
                    self.drag = Some((id, x - w.x, y - w.y));
                    return None;
                }
                Some((id, Part::Close)) => {
                    self.raise(id);
                    kill = self.request_close(id);
                    return kill;
                }
                Some((id, Part::Client)) => {
                    self.raise(id);
                    self.capture = Some(id);
                }
                None => {}
            }
        }
        // 中身へ届ける: ボタンを押したウィンドウ（離すまで）か、カーソルの下にある手前のウィンドウ
        let target = self.capture.or_else(|| match self.hit(x, y) {
            Some((id, Part::Client)) if self.focused() == Some(id) => Some(id),
            _ => None,
        });
        if let Some(id) = target {
            if let Some(w) = self.windows.get_mut(&id) {
                let ev = MouseEvent {
                    x: (x - w.x).clamp(0, w.w as i32 - 1),
                    y: (y - w.y).clamp(0, w.h as i32 - 1),
                    buttons,
                };
                push_mouse(w, ev);
            }
        }
        if !left {
            self.capture = None;
        }
        kill
    }

    /// [x] が押された。gfx_open の画面なら持ち主のプロセスを止める（その ID を返す）
    fn request_close(&mut self, id: WinId) -> Option<TaskId> {
        let w = self.windows.get_mut(&id)?;
        w.close_requested = true;
        if w.kill_on_close { w.owner } else { None }
    }

    /// 入力スレッドから: キーをフォーカスのあるウィンドウへ
    fn on_key(&mut self, ev: KeyEvent) {
        let target = match self.mode {
            Mode::Fullscreen => self.foreground(),
            Mode::Desktop => self.focused().unwrap_or(CONSOLE),
        };
        if let Some(w) = self.windows.get_mut(&target) {
            if !w.keys.push(ev) {
                klog!("gui: key queue of window {} is full, key dropped", target);
            }
        }
    }

    /// Ctrl+C で止める相手: フォーカスのあるウィンドウがアプリのものならその持ち主
    pub fn focused_owner(&self) -> Option<TaskId> {
        let id = match self.mode {
            Mode::Fullscreen => self.foreground(),
            Mode::Desktop => self.focused()?,
        };
        if id == CONSOLE { None } else { self.windows.get(&id)?.owner }
    }

    /// パニックのとき: 全画面のコンソールに戻す
    pub fn panic_takeover(&mut self) {
        self.mode = Mode::Fullscreen;
        self.console_w = self.width;
        self.console_h = self.height;
        self.order.retain(|&w| w != CONSOLE);
        self.order.push(CONSOLE);
        self.cursor_visible = false;
        if let Some(c) = self.windows.get_mut(&CONSOLE) {
            c.w = self.width;
            c.h = self.height;
        }
        vga::set_palette(&palette::standard());
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 書き換えた行を重ね合わせて VRAM へ送る
    pub fn present(&mut self) {
        if self.dirty_lo >= self.dirty_hi {
            return;
        }
        let mut row = [0u8; MAX_WIDTH];
        for y in self.dirty_lo..self.dirty_hi {
            let row = &mut row[..self.width];
            self.compose_row(y, row);
            if self.cursor_visible {
                self.draw_cursor_row(y, row);
            }
            vga::write_row(y, row);
        }
        self.dirty_lo = 0;
        self.dirty_hi = 0;
    }

    fn client_row(&self, id: WinId, y: usize) -> &[u8] {
        let w = &self.windows[&id];
        match &w.buf {
            Some(b) => &b[y * w.w..(y + 1) * w.w],
            None => {
                // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る（今ロックを持っている）
                let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
                &buf[y * self.console_w..(y + 1) * self.console_w]
            }
        }
    }

    fn compose_row(&self, y: usize, row: &mut [u8]) {
        match self.mode {
            Mode::Fullscreen => {
                row.fill(palette::BLACK);
                let fg = self.foreground();
                if fg == CONSOLE || !self.windows.contains_key(&fg) {
                    // SAFETY: 同上
                    let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
                    let w = self.console_w.min(row.len());
                    if y < self.console_h {
                        row[..w].copy_from_slice(&buf[y * self.console_w..y * self.console_w + w]);
                    }
                } else {
                    let win = &self.windows[&fg];
                    if y < win.h {
                        let n = win.w.min(row.len());
                        row[..n].copy_from_slice(&self.client_row(fg, y)[..n]);
                    }
                }
            }
            Mode::Desktop => {
                if y < BAR_H {
                    // SAFETY: 同上
                    let bar = unsafe { &*BAR_BUF.as_ptr() };
                    row.copy_from_slice(&bar[y * self.width..(y + 1) * self.width]);
                    return;
                }
                row.fill(DESKTOP_BG);
                for &id in &self.order {
                    self.draw_window_row(id, y as i32, row);
                }
            }
        }
    }

    fn draw_window_row(&self, id: WinId, y: i32, row: &mut [u8]) {
        let w = &self.windows[&id];
        let top = w.y - TITLE_H as i32 - 1;
        let bottom = w.y + w.h as i32;
        if y < top || y > bottom {
            return;
        }
        let (left, right) = (w.x - 1, w.x + w.w as i32);
        let width = row.len() as i32;
        let mut put = |x: i32, c: u8| {
            if (0..width).contains(&x) {
                row[x as usize] = c;
            }
        };
        if y == top || y == bottom {
            for x in left..=right {
                put(x, BORDER);
            }
            return;
        }
        put(left, BORDER);
        put(right, BORDER);
        let src: &[u8] = if y < w.y {
            let r = (y - top - 1) as usize;
            &w.titlebar[r * w.w..(r + 1) * w.w]
        } else {
            self.client_row(id, (y - w.y) as usize)
        };
        // 画面からはみ出す部分を切る
        let x0 = w.x.max(0);
        let x1 = (w.x + w.w as i32).min(width);
        if x0 < x1 {
            let s0 = (x0 - w.x) as usize;
            let src = &src[s0..s0 + (x1 - x0) as usize];
            let dst = &mut row[x0 as usize..x1 as usize];
            match (&w.remap, y >= w.y) {
                // 独自のパレットの中身は標準のパレットの近い色に置き換える
                (Some(map), true) => dst.iter_mut().zip(src).for_each(|(d, &s)| *d = map[s as usize]),
                _ => dst.copy_from_slice(src),
            }
        }
    }

    fn draw_cursor_row(&self, y: usize, row: &mut [u8]) {
        let dy = y as i32 - self.cursor_y;
        if dy < 0 || dy >= CURSOR_SHAPE.len() as i32 {
            return;
        }
        for (dx, c) in CURSOR_SHAPE[dy as usize].bytes().enumerate() {
            let x = self.cursor_x + dx as i32;
            let color = match c {
                b'#' => palette::CURSOR_BLACK,
                b'o' => palette::CURSOR_WHITE,
                _ => continue,
            };
            if (0..row.len() as i32).contains(&x) {
                row[x as usize] = color;
            }
        }
    }
}

fn push_mouse(w: &mut Window, ev: MouseEvent) {
    if !w.mouse.push(ev) {
        w.mouse.pop();
        w.mouse.push(ev);
    }
}

/// タイトルバーを描く（フォーカスがあれば青、なければ灰色。閉じられるなら右端に [x]）
fn render_titlebar(w: &mut Window, focused: bool) {
    w.titlebar.resize(w.w * TITLE_H, 0);
    let mut s = Surface::new(&mut w.titlebar, w.w, TITLE_H);
    s.clear(if focused { TITLE_ACTIVE } else { TITLE_INACTIVE });
    let room = w.w.saturating_sub(8 + CLOSE_W + 4) / FONT_8X8.width;
    let title: String = w.title.chars().take(room).collect();
    s.draw_text(&FONT_8X8, 4, 2, &title, palette::WHITE, None);
    if w.closable {
        let x = (w.w - CLOSE_W - 2) as i32;
        s.fill_rect(x, 1, CLOSE_W as i32, CLOSE_W as i32, palette::LIGHT_GRAY);
        s.line(x + 2, 3, x + CLOSE_W as i32 - 3, CLOSE_W as i32 - 2, palette::BLACK);
        s.line(x + CLOSE_W as i32 - 3, 3, x + 2, CLOSE_W as i32 - 2, palette::BLACK);
    }
}

/// マウスカーソル（矢印）。'#' = 黒の縁, 'o' = 白
const CURSOR_SHAPE: [&str; 12] = [
    "#         ",
    "##        ",
    "#o#       ",
    "#oo#      ",
    "#ooo#     ",
    "#oooo#    ",
    "#ooooo#   ",
    "#oooooo#  ",
    "#ooo####  ",
    "#o#o#     ",
    "## #o#    ",
    "    ##    ",
];

// --- 入力 ---------------------------------------------------------------------

/// マウスのスレッドから
pub fn on_mouse(x: i32, y: i32, buttons: u8) {
    let kill = {
        let mut s = SCREEN.lock();
        let k = s.on_mouse(x, y, buttons);
        s.present();
        k
    };
    if let Some(pid) = kill {
        crate::process::request_kill(pid);
    }
    INPUT_WAIT.notify_all();
}

/// キーボードのスレッドから
pub fn on_key(ev: KeyEvent) {
    SCREEN.lock().on_key(ev);
    INPUT_WAIT.notify_all();
}

/// 待っている人を起こす（プロセスを止めるときなど）
pub fn wake_readers() {
    INPUT_WAIT.notify_all();
}

/// キーボードの読み取りを振り分けるスレッドを作る
pub fn start_input_router() {
    crate::task::spawn("keys", || {
        loop {
            let ev = crate::drivers::keyboard::read_event();
            on_key(ev);
        }
    });
}

/// ウィンドウ `id` のキーを 1 つ取り出す（待たない）
pub fn try_read_key(id: WinId) -> Option<KeyEvent> {
    SCREEN.lock().windows.get_mut(&id)?.keys.pop()
}

/// ウィンドウ `id` のキーが来るまで待つ。ウィンドウが無くなったか閉じるよう頼まれたら None
pub fn read_key(id: WinId) -> Option<KeyEvent> {
    loop {
        {
            let mut s = SCREEN.lock();
            let w = s.windows.get_mut(&id)?;
            if let Some(k) = w.keys.pop() {
                return Some(k);
            }
            if w.close_requested {
                return None;
            }
        }
        INPUT_WAIT.wait_until(|| {
            let s = SCREEN.lock();
            s.windows.get(&id).is_none_or(|w| !w.keys.is_empty() || w.close_requested)
        });
    }
}

/// ウィンドウ `id` のマウスのイベントを 1 つ取り出す（待たない）
pub fn poll_mouse(id: WinId) -> Option<MouseEvent> {
    SCREEN.lock().windows.get_mut(&id)?.mouse.pop()
}

/// 前のマウスのイベントを捨てる
pub fn clear_mouse(id: WinId) {
    if let Some(w) = SCREEN.lock().windows.get_mut(&id) {
        while w.mouse.pop().is_some() {}
    }
}
