//! ウィンドウの重ね合わせ（コンポジタ）と入力の振り分け
//!
//! 2 つのモードがある:
//! - **全画面**（320x200）: 前面のウィンドウ 1 つだけを画面いっぱいに出す（これまでの動き）
//! - **デスクトップ**（640x480）: 上端のバー（メニュー・ウィンドウのボタン。bar.rs）、背景、重なった
//!   ウィンドウ（タイトルバーと枠付き）を出す。クリックで前に出す（フォーカス）、タイトルバーのドラッグで
//!   動かす、[_] で最小化、[x] で閉じる。Alt+Tab でウィンドウを順に前に出す（全画面でも）
//!
//! 端末（シェルの画面）もウィンドウの 1 つ。0 番のウィンドウは最初の端末（コンソール）で、バッファは
//! 静的な領域（起動直後から使うため）。ほかの端末とアプリのウィンドウのバッファはヒープから取る。
//! ウィンドウはそれぞれ「属する端末」を持つ（作ったプロセスの端末）。全画面では Alt+F1〜F4 で
//! 端末を切り替え、その端末に属するウィンドウをまとめて前に出す。
//! 描画は各ウィンドウのバッファに行い、`present` で書き換えた行だけを重ね合わせて VRAM に送る。
//! マウスカーソルは重ね合わせのときに上に描く。全画面では、前面が端末なら一番上の行にステータスバーを出す。
//!
//! パレット: 全画面では前面のウィンドウの独自のパレットを VGA にそのまま設定する。デスクトップでは
//! 他のウィンドウの色が変わらないよう VGA は標準のパレットのまま、独自のパレットを持つウィンドウの
//! 中身は重ね合わせるときに標準のパレットの近い色に置き換える。
//!
//! 入力: キーはフォーカスのあるウィンドウ（全画面では前面）に、マウスはその中の座標にして届ける。
//! ウィンドウの持ち主はそれぞれのキューから読む（`read_key` / `poll_mouse`）。

mod bar;

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
/// 全画面のときのステータスバーの高さ（1 行）
pub const FS_BAR_H: usize = 8;
const CLOSE_W: usize = 9;
pub const DESKTOP_BG: u8 = palette::CUBE_BASE + 13; // 落ち着いた青緑（カラーキューブの 0,2,1）
const BORDER: u8 = palette::LIGHT_GRAY;
const TITLE_ACTIVE: u8 = palette::BLUE;
const TITLE_INACTIVE: u8 = palette::DARK_GRAY;
/// 端末のウィンドウの大きさ（文字数）。左下に置き、上をアプリのウィンドウのために空ける
const TERMINAL_COLS: usize = 64;
const TERMINAL_ROWS: usize = 32;

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
    /// 端末のウィンドウか
    terminal: bool,
    /// 属する端末（端末のウィンドウなら自分）
    tty: Option<WinId>,
    /// 端末のとき: シェルが終わるのを待っているプロセス（Ctrl+C で止める相手）
    fg_proc: Option<TaskId>,
    /// 最小化している（バーのボタンだけになる）
    minimized: bool,
}

/// マウスの操作の結果、ロックの外で行うこと
enum MouseAction {
    /// プロセスを止める（gfx_open の画面の [x]）
    Kill(TaskId),
    /// 端末を閉じる（シェルとその端末のプロセスを止める）
    CloseTerminal(WinId),
    /// メニューを開く（項目を読む）
    OpenMenu,
    /// メニューの項目を起動する
    Launch(String),
    /// 端末のウィンドウの中身でのマウス（中身の座標とボタン）
    Terminal(WinId, i32, i32, u8),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Title,
    Minimize,
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
    /// コンソールの中身の上端（ウィンドウを登録する前に使う）
    console_y: usize,
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
    /// デスクトップのバーの右に出す状態の表示
    status_text: String,
    /// バーを描き直す
    bar_dirty: bool,
    menu: Option<bar::Menu>,
    /// デスクトップの背景の画像（バーの下の全体、画面の幅 × (高さ − BAR_H)）
    wallpaper: Option<Box<[u8]>>,
}

/// 画面。コンソール（gfx::console の CONSOLE）と両方取るときはコンソールを先に取ること
pub static SCREEN: IrqMutex<Screen> = IrqMutex::new(Screen {
    mode: Mode::Fullscreen,
    width: 0,
    height: 0,
    console_w: 0,
    console_h: 0,
    console_y: 0,
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
    status_text: String::new(),
    bar_dirty: false,
    menu: None,
    wallpaper: None,
});

/// キー・マウス・閉じる要求が届いたら起こされる
static INPUT_WAIT: WaitQueue = WaitQueue::new();

/// 起動直後（vga::init の後）に呼ぶ。まずはコンソールを画面いっぱい（一番上の行はステータスバー）に出す
pub fn init() {
    let mut s = SCREEN.lock();
    s.width = vga::width();
    s.height = vga::height();
    s.console_w = s.width;
    s.console_h = s.height - FS_BAR_H;
    s.console_y = FS_BAR_H;
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
    let mut win = new_window(w, h, None, "Terminal 1", None, false, false);
    win.y = s.console_y as i32;
    win.terminal = true;
    win.tty = Some(CONSOLE);
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
    let y = (s.height - ch - 8) as i32;
    s.console_y = y as usize;
    if let Some(c) = s.windows.get_mut(&CONSOLE) {
        c.w = cw;
        c.h = ch;
        c.x = 8;
        c.y = y;
        c.closable = false;
    }
    s.surface().clear(palette::BLACK);
    s.refresh_titlebars();
    s.cursor_visible = true;
    s.bar_dirty = true;
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
        terminal: false,
        tty: None,
        fg_proc: None,
        minimized: false,
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

    /// ステータスバー（デスクトップでは上端のバー、全画面では一番上の行）に描く
    pub fn draw_status_bar(&mut self, draw: impl FnOnce(&mut Surface)) {
        let h = match self.mode {
            Mode::Desktop => BAR_H,
            Mode::Fullscreen => FS_BAR_H,
        };
        let w = self.width;
        // SAFETY: BAR_BUF は SCREEN のロックの中でだけ触る
        let bar = unsafe { &mut (&mut *BAR_BUF.as_ptr())[..w * h] };
        draw(&mut Surface::new(bar, w, h));
        self.mark_rows(0, h);
    }

    /// ウィンドウを作って前に出す。`kill_on_close` なら [x] で持ち主を止め、そうでなければ閉じる要求のイベントを届ける。
    /// `tty` は属する端末（作ったプロセスの端末）
    pub fn create_window(
        &mut self,
        w: usize,
        h: usize,
        title: &str,
        owner: Option<TaskId>,
        kill_on_close: bool,
        tty: Option<WinId>,
    ) -> WinId {
        let id = self.add_window(w, h, title, owner, kill_on_close);
        if let Some(win) = self.windows.get_mut(&id) {
            win.tty = tty;
        }
        id
    }

    /// 端末のウィンドウを作って前に出す。全画面では画面いっぱい（一番上の行を除く）、デスクトップでは
    /// 64 桁 × 32 行
    pub fn create_terminal(&mut self, title: &str) -> WinId {
        let (w, h) = match self.mode {
            Mode::Fullscreen => (self.width, self.height - FS_BAR_H),
            Mode::Desktop => self.desktop_terminal_size(),
        };
        let id = self.add_window(w, h, title, None, false);
        if let Some(win) = self.windows.get_mut(&id) {
            win.terminal = true;
            win.tty = Some(id);
            if self.mode == Mode::Fullscreen {
                win.y = FS_BAR_H as i32;
            }
        }
        id
    }

    fn desktop_terminal_size(&self) -> (usize, usize) {
        let (cw, ch) = (TERMINAL_COLS * FONT_8X8.width, TERMINAL_ROWS * FONT_8X8.height);
        (cw.min(self.width - 2), ch.min(self.height - BAR_H - TITLE_H - 2))
    }

    fn add_window(&mut self, w: usize, h: usize, title: &str, owner: Option<TaskId>, kill_on_close: bool) -> WinId {
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
        if self.mode == Mode::Fullscreen && self.foreground() != id {
            return; // 見えていない
        }
        let top = match self.windows.get(&id) {
            Some(win) => win.y,
            // まだ登録していないコンソール（起動直後）
            None if id == CONSOLE => self.console_y as i32,
            None => return,
        };
        let bottom = top + y1 as i32;
        if bottom > 0 {
            self.mark_rows((top + y0 as i32).max(0) as usize, bottom as usize);
        }
    }

    /// デスクトップの背景の画像を変える（None で単色に戻す）。大きさは `wallpaper_size`
    pub fn set_wallpaper(&mut self, pixels: Option<Box<[u8]>>) {
        let (w, h) = self.wallpaper_size();
        self.wallpaper = pixels.filter(|p| p.len() == w * h);
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 背景の画像の大きさ（バーの下の全体）
    pub fn wallpaper_size(&self) -> (usize, usize) {
        (self.width, self.height - BAR_H)
    }

    /// ウィンドウの中身の大きさ
    pub fn window_size(&self, id: WinId) -> Option<(usize, usize)> {
        match self.windows.get(&id) {
            Some(w) => Some((w.w, w.h)),
            None if id == CONSOLE => Some((self.console_w, self.console_h)),
            None => None,
        }
    }

    /// 端末 `tty` でシェルが終わるのを待っているプロセス（Ctrl+C で止める相手）を設定する
    pub fn set_terminal_foreground(&mut self, tty: WinId, proc: Option<TaskId>) {
        if let Some(w) = self.windows.get_mut(&tty) {
            w.fg_proc = proc;
        }
    }

    /// 端末 `tty` に属するウィンドウを、重なりの順を保ったまままとめて前に出す
    pub fn raise_tty(&mut self, tty: WinId) {
        let (mut mine, others): (Vec<WinId>, Vec<WinId>) =
            self.order.iter().partition(|id| self.windows.get(id).is_some_and(|w| w.tty == Some(tty)));
        if mine.is_empty() {
            return;
        }
        let mut order = others;
        order.append(&mut mine);
        if order != self.order {
            self.order = order;
            self.focus_changed();
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

    /// キーを受け取るウィンドウ（最小化していない一番手前のもの）
    fn focused(&self) -> Option<WinId> {
        self.order.iter().rev().copied().find(|id| self.windows.get(id).is_some_and(|w| !w.minimized))
    }

    /// 前に出す（最小化していれば戻す）
    fn raise(&mut self, id: WinId) {
        let Some(w) = self.windows.get_mut(&id) else { return };
        let was_minimized = core::mem::take(&mut w.minimized);
        if !was_minimized && self.order.last() == Some(&id) {
            return;
        }
        self.order.retain(|&w| w != id);
        self.order.push(id);
        self.focus_changed();
    }

    /// 最小化する（デスクトップだけ）
    fn minimize(&mut self, id: WinId) {
        if self.mode != Mode::Desktop {
            return;
        }
        let Some(w) = self.windows.get_mut(&id) else { return };
        w.minimized = true;
        if self.drag.is_some_and(|d| d.0 == id) {
            self.drag = None;
        }
        if self.capture == Some(id) {
            self.capture = None;
        }
        self.focus_changed();
    }

    /// Alt+Tab: 一番奥の（最小化していない）ウィンドウを前に出す。前に出せるものが 1 つしか
    /// なければ、最小化したものを戻す
    fn cycle_windows(&mut self) {
        let visible: Vec<WinId> =
            self.order.iter().copied().filter(|id| self.windows.get(id).is_some_and(|w| !w.minimized)).collect();
        let next = if visible.len() >= 2 {
            Some(visible[0])
        } else {
            self.order.iter().copied().find(|id| self.windows.get(id).is_some_and(|w| w.minimized))
        };
        if let Some(id) = next {
            self.raise(id);
        }
    }

    /// 重なりの順やフォーカスが変わった: タイトルバー・パレット・バーを直して全体を描き直す
    fn focus_changed(&mut self) {
        self.refresh_titlebars();
        self.apply_palette();
        self.bar_dirty = true;
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
            if w.minimized {
                continue;
            }
            let (left, right) = (w.x - 1, w.x + w.w as i32);
            let (top, bottom) = (w.y - TITLE_H as i32 - 1, w.y + w.h as i32);
            if x < left || x > right || y < top || y > bottom {
                continue;
            }
            if y < w.y {
                let on = |bx: Option<i32>| bx.is_some_and(|bx| x >= w.x + bx && x < w.x + bx + CLOSE_W as i32);
                if on(w.close_box_x()) {
                    return Some((id, Part::Close));
                }
                if on(Some(w.minimize_box_x())) {
                    return Some((id, Part::Minimize));
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
    fn on_mouse(&mut self, x: i32, y: i32, buttons: u8) -> Option<MouseAction> {
        self.set_cursor(x, y);
        let left = buttons & 1 != 0;
        let was_left = self.buttons & 1 != 0;
        self.buttons = buttons;

        if self.mode == Mode::Fullscreen {
            let fg = self.foreground();
            if let Some(w) = self.windows.get_mut(&fg) {
                let ev = MouseEvent { x: x.clamp(0, w.w as i32 - 1), y: y.clamp(0, w.h as i32 - 1), buttons };
                push_mouse(w, ev);
            }
            return None;
        }

        if self.menu.is_some() {
            self.menu_hover(x, y);
            if left && !was_left {
                // メニューが開いていれば、どこを押しても閉じる（項目なら起動する）
                let item = self.menu_item_at(x, y).flatten();
                let name = item.and_then(|i| self.menu.as_ref().map(|m| m.items[i].clone()));
                self.close_menu();
                return name.map(MouseAction::Launch);
            }
            return None;
        }

        if let Some((id, dx, dy)) = self.drag {
            if left {
                self.move_window(id, x - dx, y - dy);
                return None;
            }
            self.drag = None;
        } else if left && !was_left && y < BAR_H as i32 && self.capture.is_none() {
            match self.bar_hit(x) {
                Some(bar::BarHit::Menu) => return Some(MouseAction::OpenMenu),
                // フォーカスのあるウィンドウのボタンなら最小化、ほかは前に出す（最小化していれば戻す）
                Some(bar::BarHit::Window(id)) if self.focused() == Some(id) => self.minimize(id),
                Some(bar::BarHit::Window(id)) => self.raise(id),
                None => {}
            }
            return None;
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
                    return self.request_close(id);
                }
                Some((id, Part::Minimize)) => {
                    self.minimize(id);
                    return None;
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
        let mut action = None;
        if let Some(id) = target {
            if let Some(w) = self.windows.get_mut(&id) {
                let ev = MouseEvent {
                    x: (x - w.x).clamp(0, w.w as i32 - 1),
                    y: (y - w.y).clamp(0, w.h as i32 - 1),
                    buttons,
                };
                // 端末は文字を選ぶのに使う（ロックの外で gfx::console に渡す）
                if w.terminal {
                    action = Some(MouseAction::Terminal(id, ev.x, ev.y, buttons));
                } else {
                    push_mouse(w, ev);
                }
            }
        }
        if !left {
            self.capture = None;
        }
        action
    }

    /// [x] が押された。gfx_open の画面なら持ち主のプロセスを、端末ならその端末のプロセスを止める
    fn request_close(&mut self, id: WinId) -> Option<MouseAction> {
        let w = self.windows.get_mut(&id)?;
        w.close_requested = true;
        if w.terminal {
            Some(MouseAction::CloseTerminal(id))
        } else if w.kill_on_close {
            w.owner.map(MouseAction::Kill)
        } else {
            None
        }
    }

    /// 入力スレッドから: キーをフォーカスのあるウィンドウへ
    fn on_key(&mut self, ev: KeyEvent) {
        // Alt+Tab: ウィンドウの切り替え（押す・離すのどちらもアプリには渡さない）
        if ev.key == KeyCode::TAB && ev.mods.alt() {
            if ev.pressed {
                self.cycle_windows();
            }
            return;
        }
        let Some(target) = self.focused() else { return };
        if let Some(w) = self.windows.get_mut(&target) {
            // 端末の Ctrl+C は、シェルが待っている前面のプロセスを止めるのに使った（キーボードの割り込みで）。
            // 入力としては渡さない（後でシェルが ^C として読んでしまわないように）
            if w.terminal && ev.ch == Some('\x03') && w.fg_proc.is_some() {
                return;
            }
            if !w.keys.push(ev) {
                klog!("gui: key queue of window {} is full, key dropped", target);
            }
        }
    }

    /// Ctrl+C で止める相手: フォーカスのあるウィンドウが端末ならシェルが待っているプロセス、
    /// アプリのものならその持ち主
    pub fn interrupt_target(&self) -> Option<TaskId> {
        let w = self.windows.get(&self.focused()?)?;
        if w.terminal { w.fg_proc } else { w.owner }
    }

    /// パニックのとき: 全画面のコンソールに戻す
    pub fn panic_takeover(&mut self) {
        self.mode = Mode::Fullscreen;
        self.console_w = self.width;
        self.console_h = self.height;
        self.console_y = 0;
        self.order.retain(|&w| w != CONSOLE);
        self.order.push(CONSOLE);
        self.cursor_visible = false;
        if let Some(c) = self.windows.get_mut(&CONSOLE) {
            c.w = self.width;
            c.h = self.height;
            c.x = 0;
            c.y = 0;
        }
        vga::set_palette(&palette::standard());
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 書き換えた行を重ね合わせて VRAM へ送る
    pub fn present(&mut self) {
        if self.bar_dirty && self.mode == Mode::Desktop {
            self.render_bar();
        }
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
                let top = match self.windows.get(&fg) {
                    Some(win) => win.y.max(0) as usize,
                    None => self.console_y,
                };
                if y < top {
                    // 端末の上の行はステータスバー
                    // SAFETY: 同上
                    let bar = unsafe { &*BAR_BUF.as_ptr() };
                    if y < FS_BAR_H {
                        row.copy_from_slice(&bar[y * self.width..(y + 1) * self.width]);
                    }
                } else if let Some(win) = self.windows.get(&fg) {
                    if y - top < win.h {
                        let n = win.w.min(row.len());
                        row[..n].copy_from_slice(&self.client_row(fg, y - top)[..n]);
                    }
                } else if y - top < self.console_h {
                    // まだウィンドウを登録していない（起動直後）
                    // SAFETY: 同上
                    let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
                    let (r, w) = (y - top, self.console_w.min(row.len()));
                    row[..w].copy_from_slice(&buf[r * self.console_w..r * self.console_w + w]);
                }
            }
            Mode::Desktop => {
                if y < BAR_H {
                    // SAFETY: 同上
                    let bar = unsafe { &*BAR_BUF.as_ptr() };
                    row.copy_from_slice(&bar[y * self.width..(y + 1) * self.width]);
                    return;
                }
                match &self.wallpaper {
                    Some(wp) => {
                        let r = y - BAR_H;
                        row.copy_from_slice(&wp[r * self.width..(r + 1) * self.width]);
                    }
                    None => row.fill(DESKTOP_BG),
                }
                for &id in &self.order {
                    self.draw_window_row(id, y as i32, row);
                }
                self.draw_menu_row(y, row);
            }
        }
    }

    fn draw_window_row(&self, id: WinId, y: i32, row: &mut [u8]) {
        let w = &self.windows[&id];
        if w.minimized {
            return;
        }
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

impl Window {
    /// タイトルバーの [x] の左端（中身の左端から）。閉じられなければ None
    fn close_box_x(&self) -> Option<i32> {
        self.closable.then(|| (self.w - CLOSE_W - 2) as i32)
    }

    /// タイトルバーの [_] の左端（[x] の左、[x] が無ければ右端）
    fn minimize_box_x(&self) -> i32 {
        let x = (self.w - CLOSE_W - 2) as i32;
        if self.closable { x - CLOSE_W as i32 - 2 } else { x }
    }
}

/// タイトルバーを描く（フォーカスがあれば青、なければ灰色。右端に [_] と、閉じられるなら [x]）
fn render_titlebar(w: &mut Window, focused: bool) {
    let close_x = w.close_box_x();
    let min_x = w.minimize_box_x();
    w.titlebar.resize(w.w * TITLE_H, 0);
    let mut s = Surface::new(&mut w.titlebar, w.w, TITLE_H);
    s.clear(if focused { TITLE_ACTIVE } else { TITLE_INACTIVE });
    let room = (min_x - 8).max(0) as usize / FONT_8X8.width;
    let title: String = w.title.chars().take(room).collect();
    s.draw_text(&FONT_8X8, 4, 2, &title, palette::WHITE, None);
    let c = CLOSE_W as i32;
    s.fill_rect(min_x, 1, c, c, palette::LIGHT_GRAY);
    s.hline(min_x + 2, c - 2, c - 4, palette::BLACK);
    if let Some(x) = close_x {
        s.fill_rect(x, 1, c, c, palette::LIGHT_GRAY);
        s.line(x + 2, 3, x + c - 3, c - 2, palette::BLACK);
        s.line(x + c - 3, 3, x + 2, c - 2, palette::BLACK);
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
    let action = {
        let mut s = SCREEN.lock();
        let a = s.on_mouse(x, y, buttons);
        s.present();
        a
    };
    match action {
        Some(MouseAction::Kill(pid)) => crate::process::request_kill(pid),
        Some(MouseAction::CloseTerminal(tty)) => crate::process::kill_tty(tty),
        Some(MouseAction::OpenMenu) => {
            let items = menu_items();
            let mut s = SCREEN.lock();
            s.open_menu(items);
            s.present();
        }
        Some(MouseAction::Launch(name)) => launch(&name),
        Some(MouseAction::Terminal(tty, x, y, b)) => crate::gfx::console::mouse(tty, x, y, b),
        None => {}
    }
    INPUT_WAIT.notify_all();
}

/// メニューの項目: "Terminal" と /BIN のアプリ（小文字の名前、拡張子なし）
fn menu_items() -> Vec<String> {
    let mut apps: Vec<String> = crate::fs::read_dir("/BIN")
        .map(|entries| {
            entries
                .iter()
                .filter(|e| !e.is_dir())
                .filter_map(|e| e.name.strip_suffix(".ELF").map(|n| n.to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    apps.sort();
    let mut items = vec![String::from(MENU_TERMINAL)];
    items.append(&mut apps);
    items
}

const MENU_TERMINAL: &str = "Terminal";

/// メニューから起動する。アプリは最初の端末につながる（出力はそこに出る）。終わるのは誰も待たない
fn launch(name: &str) {
    if name == MENU_TERMINAL {
        crate::gfx::console::open_terminal(None, None);
        return;
    }
    let path = crate::process::resolve_program(name);
    match crate::process::spawn(&path, &[name]) {
        Ok(id) => {
            crate::process::detach(id);
            klog!("menu: started {name} ({id})");
        }
        Err(e) => klog!("menu: {name}: {}", vibe_abi::err::name(e)),
    }
}

/// キーボードのスレッドから
pub fn on_key(ev: KeyEvent) {
    // Alt+F1〜F4: 端末の切り替え（無ければ作る）。押す・離すのどちらもアプリには渡さない
    if ev.mods.alt() {
        if let Some(n @ 1..=4) = ev.key.function_number() {
            if ev.pressed {
                crate::gfx::console::switch_to(n as usize);
            }
            return;
        }
    }
    {
        let mut s = SCREEN.lock();
        s.on_key(ev);
        s.present();
    }
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

/// ウィンドウ `id` のキーが来るまで待つ。ウィンドウが無くなったか閉じるよう頼まれたか、
/// 自分（プロセス）が止めるよう頼まれたら None
pub fn read_key(id: WinId) -> Option<KeyEvent> {
    let me = crate::task::current_id();
    loop {
        if crate::process::kill_pending(me) {
            return None;
        }
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
                || crate::process::kill_pending(me)
        });
    }
}

/// ウィンドウが無くなったか、閉じるよう頼まれている（端末のシェルが終わる合図）
pub fn is_closing(id: WinId) -> bool {
    SCREEN.lock().windows.get(&id).is_none_or(|w| w.close_requested)
}

/// ウィンドウの持ち主か
pub fn is_owner(id: WinId, task: TaskId) -> bool {
    SCREEN.lock().windows.get(&id).is_some_and(|w| w.owner == Some(task))
}

/// ウィンドウのイベント
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Close,
}

fn take_event(w: &mut Window) -> Option<Event> {
    if w.close_requested && !w.kill_on_close {
        // 閉じる要求は 1 回だけ届ける（アプリが閉じなければ、また [x] で届く）
        w.close_requested = false;
        return Some(Event::Close);
    }
    if let Some(k) = w.keys.pop() {
        return Some(Event::Key(k));
    }
    w.mouse.pop().map(Event::Mouse)
}

/// ウィンドウ `id` のイベントを待つ（閉じる要求・キー・マウスの順）。`timeout_ticks` が None なら
/// いつまでも待つ。時間切れかウィンドウが無くなるか、自分が止めるよう頼まれたら None
pub fn next_event(id: WinId, timeout_ticks: Option<u32>) -> Option<Event> {
    let me = crate::task::current_id();
    let start = crate::drivers::pit::ticks();
    let timed_out = || {
        timeout_ticks.is_some_and(|t| crate::drivers::pit::ticks().wrapping_sub(start) >= t)
            || crate::process::kill_pending(me)
    };
    loop {
        {
            let mut s = SCREEN.lock();
            let w = s.windows.get_mut(&id)?;
            if let Some(e) = take_event(w) {
                return Some(e);
            }
        }
        if timed_out() {
            return None;
        }
        let has_event = || {
            let s = SCREEN.lock();
            s.windows.get(&id).is_none_or(|w| {
                (w.close_requested && !w.kill_on_close) || !w.keys.is_empty() || !w.mouse.is_empty()
            })
        };
        if timeout_ticks.is_some() {
            WaitQueue::wait_until_any(&[&INPUT_WAIT, crate::drivers::pit::tick_queue()], || has_event() || timed_out());
        } else {
            INPUT_WAIT.wait_until(has_event);
        }
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
