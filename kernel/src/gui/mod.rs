//! ウィンドウの重ね合わせ（コンポジタ）と入力の振り分け
//!
//! 画面は VGA のモード 12h（640x480）の白黒（docs/plans/mono.md）で、2 つのモードがある:
//! - **全画面**: 起動直後とパニックのときだけ。最初の端末（コンソール）を画面いっぱいに出す
//! - **デスクトップ**: 上端のバー（メニュー・ウィンドウのボタン。bar.rs）、背景、重なったウィンドウ（黒い枠と
//!   タイトルバーと影付き）を出す。クリックで前に出す（フォーカス）、タイトルバーのドラッグで動かす（ドラッグの間は
//!   枠だけ）、[_] で最小化、[x] で閉じる。Alt+Tab でウィンドウを順に前に出す。見た目は draw.rs
//!
//! 端末（シェルの画面）もウィンドウの 1 つ。0 番のウィンドウは最初の端末（コンソール）で、起動直後は静的な
//! 領域を使う（デスクトップにするときにヒープのバッファに移る）。ほかの端末とアプリのウィンドウのバッファは
//! ヒープから取る。ウィンドウはそれぞれ「属する端末」を持つ（作ったプロセスの端末）。
//! 描画は各ウィンドウのバッファに行い、`present` で書き換えた矩形の行だけを重ね合わせて VRAM に送る。
//! マウスカーソルは重ね合わせのときに上に描く。
//!
//! 色: どのバッファも 1 ピクセル 1 バイトの色番号（`palette::VGA16` の 0〜15。アプリの API と同じ）。重ね合わせも
//! 色番号で行い、`vga::write_row_mono` が白黒の模様（`vibe_gfx::mono`。0 と 15 はそのまま）にして書く
//! （書き換えた x の範囲だけ）。
//!
//! 入力: キーはフォーカスのあるウィンドウに、マウスはその中の座標にして届ける。
//! ウィンドウの持ち主はそれぞれのキューから読む（`read_key` / `poll_mouse`）。

mod bar;
mod draw;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_gfx::{Surface, palette};
use vibe_keyboard::{KeyCode, KeyEvent, Modifiers};

use crate::drivers::vga;
use crate::sync::{BootCell, IrqMutex};
use crate::task::sched::TaskId;
use crate::task::wait::WaitQueue;
use core::sync::atomic::Ordering;
use crate::util::ring::Ring;

pub type WinId = u32;
pub const CONSOLE: WinId = 0;

/// タイトルバー（下の黒い線を含む）とデスクトップの上端のバーの高さ、ウィンドウの枠の太さ
pub const TITLE_H: usize = draw::TITLE_H;
pub const BAR_H: usize = draw::BAR_H;
const BORDER: i32 = draw::BORDER;
/// タイトルバーのボタン（[_]・[x]）の押せる所の幅と、右端・ボタンどうしの間
const BUTTON_W: usize = 20;
const BUTTON_GAP: usize = 2;
/// デスクトップの端末のウィンドウの大きさ（文字数）
const TERMINAL_COLS: usize = 80;
const TERMINAL_ROWS: usize = 25;
/// 画面の大きさ（モード 12h）。起動直後とパニックのときのコンソールの静的な領域と、重ね合わせの 1 行もこの大きさ
const SCREEN_W: usize = 640;
const SCREEN_H: usize = 480;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// 起動直後とパニックのとき: コンソールを画面いっぱいに
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

/// デスクトップの背景の画像（白黒に誤差拡散した色番号（0 か 15）の w × h と並べ方）
pub struct Wallpaper {
    pub w: usize,
    pub h: usize,
    pub fit: vibe_gfx::wallpaper::Fit,
    pub pixels: Box<[u8]>,
}

/// ウィンドウの中身のバッファ（色番号）
enum Pixels {
    /// 最初の端末の静的な領域（起動直後・パニックのとき）
    Static,
    Heap(Box<[u8]>),
}

struct Window {
    w: usize,
    h: usize,
    pixels: Pixels,
    /// 中身の左上の画面上の位置（デスクトップのとき）
    x: i32,
    y: i32,
    title: String,
    /// 作ったプロセスのプログラムの名前（小文字、拡張子なし。端末は "terminal"）
    app: String,
    owner: Option<TaskId>,
    closable: bool,
    /// タイトルバー（色番号、幅 × TITLE_H）
    titlebar: Vec<u8>,
    /// タイトルバー・上端のバーのボタンに出すアイコン（白黒の 16x16）
    icon: Option<&'static [u8]>,
    /// アプリが忙しい間に打たれたキーも落とさないよう、押す・離すで 128 打鍵分
    keys: Ring<KeyEvent, 256>,
    mouse: Ring<MouseEvent, 64>,
    close_requested: bool,
    /// 端末のウィンドウか
    terminal: bool,
    /// 属する端末（端末のウィンドウなら自分）
    tty: Option<WinId>,
    /// 端末のとき: シェルが終わるのを待っているプロセスの集まり（Ctrl+C で止める相手）
    fg_procs: Foreground,
    /// 最小化している（バーのボタンだけになる）
    minimized: bool,
}

/// 端末の前面のプロセスの集まり（パイプでつないだもの）。割り込みの中でも読むので固定長
#[derive(Clone, Copy)]
pub struct Foreground {
    ids: [TaskId; MAX_FOREGROUND],
    len: usize,
}

/// 端末の前面のプロセスを止めるのに使った Ctrl+C の数（そのキーを端末に渡さないため）
static CTRL_C_USED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// 前面にできるプロセスの数（パイプでつなげる数の上限）
pub const MAX_FOREGROUND: usize = 16;

impl Foreground {
    pub const EMPTY: Foreground = Foreground { ids: [0; MAX_FOREGROUND], len: 0 };

    fn from(ids: &[TaskId]) -> Foreground {
        let mut f = Foreground::EMPTY;
        f.len = ids.len().min(MAX_FOREGROUND);
        f.ids[..f.len].copy_from_slice(&ids[..f.len]);
        f
    }

    pub fn ids(&self) -> &[TaskId] {
        &self.ids[..self.len]
    }
}

/// マウスの操作の結果、ロックの外で行うこと
enum MouseAction {
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

static CONSOLE_BUF: BootCell<[u8; SCREEN_W * SCREEN_H]> = BootCell::new([0; SCREEN_W * SCREEN_H]);
/// 重ね合わせの 1 行（スタックに置くと大きいので静的な領域に）
static ROW: BootCell<[u8; SCREEN_W]> = BootCell::new([0; SCREEN_W]);

pub struct Screen {
    mode: Mode,
    width: usize,
    height: usize,
    console_w: usize,
    console_h: usize,
    /// コンソールの中身の上端（ウィンドウを登録する前に使う）
    console_y: usize,
    /// Window は大きい（キーとマウスのリングを持つ）ので Box に入れる。値のまま B 木に入れると、
    /// 取り出し・並べ替えのたびに大きな値がスタックの上で動く
    windows: BTreeMap<WinId, Box<Window>>,
    /// 重なりの順（後ろが手前）
    order: Vec<WinId>,
    next_id: WinId,
    /// 書き換えた矩形（行 [dirty_lo, dirty_hi)・x が [dirty_x0, dirty_x1)。全部をまとめた 1 つの矩形）
    dirty_lo: usize,
    dirty_hi: usize,
    dirty_x0: usize,
    dirty_x1: usize,
    cursor_x: i32,
    cursor_y: i32,
    cursor_visible: bool,
    /// タイトルバーをドラッグ中: (ウィンドウ, つかんだ位置の中身の左上からのずれ)
    drag: Option<(WinId, i32, i32)>,
    /// ドラッグしている間の枠の位置（中身の左上）。離したときにウィンドウをここへ動かす
    outline: Option<(i32, i32)>,
    /// 中身でボタンを押したウィンドウ（離すまでマウスを届ける）
    capture: Option<WinId>,
    buttons: u8,
    placed: u32,
    /// デスクトップのバーの右に出す状態の表示
    status_text: String,
    /// バーを描き直す
    bar_dirty: bool,
    menu: Option<bar::Menu>,
    /// デスクトップの背景の画像（1 行ずつ並べる）
    wallpaper: Option<Box<Wallpaper>>,
    /// デスクトップの上端のバー（色番号、画面の幅 × BAR_H）
    bar: Vec<u8>,
    /// 最後にマウスの動きを届けたウィンドウ（出たときに知らせる）
    mouse_in: Option<WinId>,
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
    dirty_x0: 0,
    dirty_x1: 0,
    cursor_x: 0,
    cursor_y: 0,
    cursor_visible: false,
    drag: None,
    outline: None,
    capture: None,
    buttons: 0,
    placed: 0,
    status_text: String::new(),
    bar_dirty: false,
    menu: None,
    wallpaper: None,
    bar: Vec::new(),
    mouse_in: None,
});

/// キー・マウス・閉じる要求が届いたら起こされる
static INPUT_WAIT: WaitQueue = WaitQueue::new();

/// 起動直後（vga::init の後）に呼ぶ。まずはコンソールを画面いっぱいに出す
pub fn init() {
    let mut s = SCREEN.lock();
    s.width = vga::width().min(SCREEN_W);
    s.height = vga::height().min(SCREEN_H);
    s.console_w = s.width;
    s.console_h = s.height;
    s.console_y = 0;
    s.cursor_x = (s.width / 2) as i32;
    s.cursor_y = (s.height / 2) as i32;
    s.surface().clear(palette::BLACK);
    let h = s.height;
    s.mark_rows(0, h);
    s.present();
}

/// ヒープが使えるようになったら呼ぶ。コンソールをウィンドウとして登録する
pub fn init_windows() {
    let mut s = SCREEN.lock();
    let (w, h) = (s.console_w, s.console_h);
    let mut win = new_window(w, h, Pixels::Static, "Terminal 1", None, false);
    win.y = s.console_y as i32;
    win.terminal = true;
    win.tty = Some(CONSOLE);
    s.windows.insert(CONSOLE, Box::new(win));
    s.order.push(CONSOLE);
}

/// デスクトップにする（スケジューラが動いた後に呼ぶ）: 最初の端末を 8x16 の 80x25 のウィンドウにし、
/// 左右の枠が画面の外に出る位置（x = 0）に置く
pub fn start_desktop() {
    let mut s = SCREEN.lock();
    s.mode = Mode::Desktop;
    let (cw, ch) = s.desktop_terminal_size();
    s.console_w = cw;
    s.console_h = ch;
    let y = (s.height - ch - 8).max(BAR_H + TITLE_H + BORDER as usize) as i32;
    s.console_y = y as usize;
    if let Some(c) = s.windows.get_mut(&CONSOLE) {
        c.icon = crate::gfx::icons::get("terminal");
        c.app = String::from("terminal");
        c.w = cw;
        c.h = ch;
        c.x = 0;
        c.y = y;
        c.closable = false;
        c.pixels = Pixels::Heap(vec![palette::BLACK; cw * ch].into_boxed_slice());
    }
    s.refresh_titlebars();
    s.cursor_visible = true;
    s.bar_dirty = true;
    let h = s.height;
    s.mark_rows(0, h);
    s.present();
}

#[allow(dead_code)]
pub fn mode() -> Mode {
    SCREEN.lock().mode
}

fn new_window(w: usize, h: usize, pixels: Pixels, title: &str, owner: Option<TaskId>, closable: bool) -> Window {
    Window {
        w,
        h,
        pixels,
        x: 0,
        y: 0,
        title: String::from(title),
        app: String::new(),
        owner,
        closable,
        titlebar: Vec::new(),
        icon: None,
        keys: Ring::new(NO_KEY),
        mouse: Ring::new(MouseEvent { x: 0, y: 0, buttons: 0 }),
        close_requested: false,
        terminal: false,
        tty: None,
        fg_procs: Foreground::EMPTY,
        minimized: false,
    }
}

impl Screen {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// コンソールの静的な領域（起動直後とパニックのときの端末の中身。パニックの画面もここに描く）
    pub fn surface(&mut self) -> Surface<'_> {
        static_surface(self.console_w.min(SCREEN_W), self.console_h.min(SCREEN_H))
    }

    /// コンソールの大きさ（ピクセル）
    pub fn console_size(&self) -> (usize, usize) {
        (self.console_w, self.console_h)
    }

    /// コンソールの行 [y0, y1) を書き換えたことを記録する
    pub fn mark_dirty(&mut self, y0: usize, y1: usize) {
        self.mark_window_dirty(CONSOLE, y0, y1);
    }

    /// ウィンドウを作って前に出す。[x] で閉じる要求のイベントを届ける。
    /// `tty` は属する端末（作ったプロセスの端末）、`app` はアイコンを選ぶアプリの名前（小文字、拡張子なし）
    pub fn create_window(
        &mut self,
        w: usize,
        h: usize,
        title: &str,
        owner: Option<TaskId>,
        tty: Option<WinId>,
        app: &str,
    ) -> WinId {
        let id = self.add_window(w, h, title, owner);
        if let Some(win) = self.windows.get_mut(&id) {
            win.tty = tty;
            win.icon = crate::gfx::icons::for_app(app);
            win.app = String::from(app);
        }
        // アイコンを付けた後でタイトルバーを描き直す
        self.refresh_titlebars();
        id
    }

    /// 端末のウィンドウ（8x16 の 80x25）を作って前に出す
    pub fn create_terminal(&mut self, title: &str) -> WinId {
        let (w, h) = self.desktop_terminal_size();
        let id = self.add_window(w, h, title, None);
        if let Some(win) = self.windows.get_mut(&id) {
            win.icon = crate::gfx::icons::get("terminal");
            win.app = String::from("terminal");
            win.terminal = true;
            win.tty = Some(id);
        }
        self.refresh_titlebars();
        id
    }

    /// デスクトップの端末の中身の大きさ（ピクセル）。幅は画面いっぱい（左右の枠は画面の外に出る）
    fn desktop_terminal_size(&self) -> (usize, usize) {
        let f = draw::font();
        let (cw, ch) = (TERMINAL_COLS * f.width, TERMINAL_ROWS * f.height);
        (cw.min(self.width), ch.min(self.height - BAR_H - TITLE_H - 2 - 8))
    }

    fn add_window(&mut self, w: usize, h: usize, title: &str, owner: Option<TaskId>) -> WinId {
        self.next_id += 1; // 0 はコンソールなので 1 から
        let id = self.next_id;
        let pixels = Pixels::Heap(vec![palette::BLACK; w * h].into_boxed_slice());
        let mut win = new_window(w, h, pixels, title, owner, true);
        if self.mode == Mode::Desktop {
            // 少しずつずらして置く。画面の幅いっぱいのもの（端末）は x = 0（左右の枠は画面の外）
            let n = (self.placed % 8) as i32;
            self.placed += 1;
            let max_x = (self.width as i32 - w as i32 - BORDER).max(0);
            win.x = (120 + n * 24).min(max_x);
            // 中身の上端は 65 から（枠の上端 43。上端のバーとの間に背景が見える）
            win.y = ((BAR_H + TITLE_H) as i32 + 25 + n * 20).min((self.height as i32 - h as i32 - BORDER).max(30));
        }
        self.windows.insert(id, Box::new(win));
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
            self.outline = None;
        }
        if self.capture == Some(id) {
            self.capture = None;
        }
        self.focus_changed();
    }

    /// ウィンドウの中身（色番号）
    pub fn window_surface(&mut self, id: WinId) -> Option<Surface<'_>> {
        let (cw, ch) = (self.console_w, self.console_h);
        match self.windows.get_mut(&id) {
            Some(win) => {
                let (w, h) = (win.w, win.h);
                Some(match &mut win.pixels {
                    Pixels::Static => static_surface(cw, ch),
                    Pixels::Heap(b) => Surface::new(b, w, h),
                })
            }
            // まだ登録していないコンソール（起動直後）
            None if id == CONSOLE => Some(static_surface(cw, ch)),
            None => None,
        }
    }

    /// ウィンドウ `id` の中身の行 [y0, y1) を書き換えたことを記録する
    pub fn mark_window_dirty(&mut self, id: WinId, y0: usize, y1: usize) {
        if self.mode == Mode::Fullscreen && self.foreground() != id {
            return; // 見えていない
        }
        let (left, right, top) = match self.windows.get(&id) {
            Some(win) => (win.x, win.x + win.w as i32, win.y),
            // まだ登録していないコンソール（起動直後）
            None if id == CONSOLE => (0, self.width as i32, self.console_y as i32),
            None => return,
        };
        let bottom = top + y1 as i32;
        if bottom > 0 {
            self.mark_rect((top + y0 as i32).max(0) as usize, bottom as usize, left.max(0) as usize, right.max(0) as usize);
        }
    }

    /// デスクトップの背景の画像を変える（None で背景の色に戻す）
    pub fn set_wallpaper(&mut self, wallpaper: Option<Wallpaper>) {
        self.wallpaper = wallpaper.filter(|w| w.pixels.len() == w.w * w.h).map(Box::new);
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 背景の画像を並べる領域の大きさ（画面全体）
    pub fn wallpaper_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// ウィンドウの中身の大きさ
    pub fn window_size(&self, id: WinId) -> Option<(usize, usize)> {
        match self.windows.get(&id) {
            Some(w) => Some((w.w, w.h)),
            None if id == CONSOLE => Some((self.console_w, self.console_h)),
            None => None,
        }
    }

    /// 端末 `tty` でシェルが終わるのを待っているプロセスの集まり（Ctrl+C で止める相手）を設定する。
    /// 空なら前面のプロセスは無い（Ctrl+C はシェルへの入力になる）
    pub fn set_terminal_foreground(&mut self, tty: WinId, procs: &[TaskId]) {
        if let Some(w) = self.windows.get_mut(&tty) {
            w.fg_procs = Foreground::from(procs);
        }
    }

    /// ウィンドウのタイトルを変える（タイトルバーと上端のバーを描き直す）
    pub fn set_window_title(&mut self, id: WinId, title: &str) {
        let Some(win) = self.windows.get_mut(&id) else { return };
        if win.title == title {
            return;
        }
        win.title = String::from(title);
        let y = win.y;
        self.refresh_titlebars();
        self.bar_dirty = true;
        self.mark_rows((y - TITLE_H as i32).max(0) as usize, y.max(0) as usize);
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
        let (x, y) = (self.cursor_x.max(0) as usize, self.cursor_y.max(0) as usize);
        self.mark_rect(y, y + CURSOR_SHAPE.len(), x, x + CURSOR_SHAPE[0].len());
    }

    /// 行 [y0, y1) の全体を書き換えた
    fn mark_rows(&mut self, y0: usize, y1: usize) {
        let w = self.width;
        self.mark_rect(y0, y1, 0, w);
    }

    /// 行 [y0, y1)・x が [x0, x1) の矩形を書き換えた（範囲は全部をまとめた 1 つの矩形で持つ）
    fn mark_rect(&mut self, y0: usize, y1: usize, x0: usize, x1: usize) {
        let (y1, x1) = (y1.min(self.height), x1.min(self.width));
        if y0 >= y1 || x0 >= x1 {
            return;
        }
        if self.dirty_lo >= self.dirty_hi {
            (self.dirty_lo, self.dirty_hi, self.dirty_x0, self.dirty_x1) = (y0, y1, x0, x1);
        } else {
            self.dirty_lo = self.dirty_lo.min(y0);
            self.dirty_hi = self.dirty_hi.max(y1);
            self.dirty_x0 = self.dirty_x0.min(x0);
            self.dirty_x1 = self.dirty_x1.max(x1);
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
            self.outline = None;
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

    /// 重なりの順やフォーカスが変わった: タイトルバー・バーを直して全体を描き直す
    fn focus_changed(&mut self) {
        self.refresh_titlebars();
        self.bar_dirty = true;
        let h = self.height;
        self.mark_rows(0, h);
    }

    fn refresh_titlebars(&mut self) {
        if self.mode != Mode::Desktop {
            return;
        }
        let focused = self.focused();
        for (&id, win) in self.windows.iter_mut() {
            draw::render_titlebar(win, Some(id) == focused);
        }
    }

    /// (x, y) にある一番手前のウィンドウとその部分
    fn hit(&self, x: i32, y: i32) -> Option<(WinId, Part)> {
        for &id in self.order.iter().rev() {
            let w = &self.windows[&id];
            if w.minimized {
                continue;
            }
            let (left, right, top, bottom) = w.frame();
            if x < left || x > right || y < top || y > bottom {
                continue;
            }
            if y < w.y {
                let on = |bx: Option<i32>| bx.is_some_and(|bx| x >= w.x + bx && x < w.x + bx + BUTTON_W as i32);
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

    /// 中身の左上を (x, y) に置くときの位置: タイトルバーが上端のバーの下に、少なくとも一部が画面に残るようにする
    fn clamp_position(&self, w: &Window, x: i32, y: i32) -> (i32, i32) {
        let (sw, sh) = (self.width as i32, self.height as i32);
        (x.clamp(-(w.w as i32) + 40, sw - 40), y.clamp((BAR_H + TITLE_H) as i32 + BORDER, sh - 4))
    }

    fn move_window(&mut self, id: WinId, x: i32, y: i32) {
        let Some(win) = self.windows.get(&id) else { return };
        let (nx, ny) = self.clamp_position(win, x, y);
        let Some(win) = self.windows.get_mut(&id) else { return };
        // 枠と影の行（影は太い方の分）
        let rows = |w: &Window| {
            let (_, _, top, bottom) = w.frame();
            (top, bottom + 1 + draw::SHADOW_FOCUSED)
        };
        let old = rows(win);
        win.x = nx;
        win.y = ny;
        let new = rows(win);
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

        if self.drag.is_some() {
            if left {
                let (_, dx, dy) = self.drag.unwrap_or_default();
                self.move_outline(x - dx, y - dy);
                return None;
            }
            self.finish_outline();
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
                    // ドラッグの間は枠だけを動かす（離したときにウィンドウを動かす）
                    self.raise(id);
                    let w = &self.windows[&id];
                    let (wx, wy) = (w.x, w.y);
                    self.drag = Some((id, x - wx, y - wy));
                    self.outline = None;
                    self.move_outline(wx, wy);
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
        // マウスがウィンドウから出たら、前のウィンドウに (-1, -1) を届ける（部品の「乗っている」を消すため）
        if self.mouse_in != target {
            if let Some(w) = self.mouse_in.and_then(|old| self.windows.get_mut(&old)) {
                if !w.terminal {
                    push_mouse(w, MouseEvent { x: -1, y: -1, buttons: 0 });
                }
            }
            self.mouse_in = target;
        }
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

    /// [x] が押された。端末ならその端末のプロセスを止める。アプリには閉じる要求のイベントを届ける
    fn request_close(&mut self, id: WinId) -> Option<MouseAction> {
        let w = self.windows.get_mut(&id)?;
        w.close_requested = true;
        w.terminal.then_some(MouseAction::CloseTerminal(id))
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
            // 入力としては渡さない（後でシェルが ^C として読んでしまわないように）。このキーが届く前に
            // シェルが前面を空に戻していることがあるので、割り込みで使った数（CTRL_C_USED）で見る
            if w.terminal && ev.ch == Some('\x03') {
                let used = ev.pressed
                    && CTRL_C_USED.try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1)).is_ok();
                if used || w.fg_procs.len > 0 {
                    return;
                }
            }
            if !w.keys.push(ev) {
                klog!("gui: key queue of window {} is full, key dropped", target);
            }
        }
    }

    /// Ctrl+C で止める相手: フォーカスのあるウィンドウが端末ならシェルが待っているプロセスの集まり、
    /// アプリのものならその持ち主（割り込みの中から呼ぶのでメモリを確保しない）。
    /// 端末の前面のプロセスを止めるのに使ったら、そのキーは端末に渡さない（`on_key`）
    pub fn interrupt_targets(&self) -> Foreground {
        let Some(w) = self.focused().and_then(|id| self.windows.get(&id)) else { return Foreground::EMPTY };
        match (w.terminal, w.owner) {
            (true, _) => {
                if w.fg_procs.len > 0 {
                    CTRL_C_USED.fetch_add(1, Ordering::AcqRel);
                }
                w.fg_procs
            }
            (false, Some(owner)) => Foreground::from(&[owner]),
            (false, None) => Foreground::EMPTY,
        }
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
        self.menu = None;
        let (cw, ch) = (self.console_w, self.console_h);
        if let Some(c) = self.windows.get_mut(&CONSOLE) {
            c.w = cw;
            c.h = ch;
            c.x = 0;
            c.y = 0;
            c.pixels = Pixels::Static;
        }
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// ウィンドウの中身の左上を (x, y) に動かす（ktest で決まった所に置くため）
    #[cfg(feature = "qemu-test")]
    pub fn place_window(&mut self, id: WinId, x: i32, y: i32) {
        self.move_window(id, x, y);
    }

    /// 画面全体を描き直すことにする（ktest で 1 枚の時間を測るため）
    #[cfg(feature = "qemu-test")]
    pub fn redraw_all(&mut self) {
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 書き換えた矩形を重ね合わせて VRAM へ送る（x は 8 ピクセル単位に広げて）
    pub fn present(&mut self) {
        if self.bar_dirty && self.mode == Mode::Desktop {
            self.render_bar();
        }
        if self.dirty_lo >= self.dirty_hi {
            return;
        }
        // SAFETY: ROW は SCREEN のロックの中（ここ）でだけ触る
        let row = unsafe { &mut (&mut *ROW.as_ptr())[..self.width] };
        let focused = self.focused();
        let (x0, x1) = (self.dirty_x0, self.dirty_x1);
        for y in self.dirty_lo..self.dirty_hi {
            match self.mode {
                Mode::Desktop => self.compose_row(y, row, focused),
                Mode::Fullscreen => self.compose_fullscreen_row(y, row),
            }
            vga::write_row_mono(y, row, x0, x1);
        }
        self.dirty_lo = 0;
        self.dirty_hi = 0;
    }

    /// ウィンドウの中身の `y` 行目
    fn client_row(&self, id: WinId, y: usize) -> &[u8] {
        let w = &self.windows[&id];
        match &w.pixels {
            Pixels::Heap(b) => &b[y * w.w..(y + 1) * w.w],
            Pixels::Static => {
                // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る（今ロックを持っている）
                let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
                &buf[y * self.console_w..(y + 1) * self.console_w]
            }
        }
    }

    /// 全画面（起動直後・パニック）の 1 行: 前面のウィンドウ（コンソール）をそのまま
    fn compose_fullscreen_row(&self, y: usize, row: &mut [u8]) {
        row.fill(palette::BLACK);
        let fg = self.foreground();
        let top = self.windows.get(&fg).map_or(self.console_y, |w| w.y.max(0) as usize);
        if y < top {
            return;
        }
        if let Some(win) = self.windows.get(&fg) {
            if y - top < win.h {
                let n = win.w.min(row.len());
                row[..n].copy_from_slice(&self.client_row(fg, y - top)[..n]);
            }
        } else if y - top < self.console_h {
            // まだウィンドウを登録していない（起動直後）
            // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る（今ロックを持っている）
            let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
            let (r, w) = (y - top, self.console_w.min(row.len()));
            row[..w].copy_from_slice(&buf[r * self.console_w..r * self.console_w + w]);
        }
        if self.cursor_visible {
            self.draw_cursor_row(y, row);
        }
    }

    /// マウスカーソル（白黒の矢印）の `y` 行目を重ねる
    fn draw_cursor_row(&self, y: usize, row: &mut [u8]) {
        let dy = y as i32 - self.cursor_y;
        if dy < 0 || dy >= CURSOR_SHAPE.len() as i32 {
            return;
        }
        for (dx, c) in CURSOR_SHAPE[dy as usize].bytes().enumerate() {
            let x = self.cursor_x + dx as i32;
            let color = match c {
                b'#' => palette::BLACK,
                b'o' => palette::WHITE,
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
    /// 枠の外側の (左, 右, 上, 下)（画面の座標。どれも枠に含む）。影はこの右下の外
    fn frame(&self) -> (i32, i32, i32, i32) {
        (
            self.x - BORDER,
            self.x + self.w as i32 + BORDER - 1,
            self.y - TITLE_H as i32 - BORDER,
            self.y + self.h as i32 + BORDER - 1,
        )
    }

    /// タイトルバーの [x] の左端（中身の左端から）。閉じられなければ None
    fn close_box_x(&self) -> Option<i32> {
        self.closable.then(|| self.w as i32 - (BUTTON_W + BUTTON_GAP) as i32)
    }

    /// タイトルバーの [_] の左端（[x] の左、[x] が無ければ右端）
    fn minimize_box_x(&self) -> i32 {
        let x = self.w as i32 - (BUTTON_W + BUTTON_GAP) as i32;
        if self.closable { x - (BUTTON_W + BUTTON_GAP) as i32 } else { x }
    }
}

/// コンソールの静的な領域（`w` × `h`。画面の大きさまで）
fn static_surface(w: usize, h: usize) -> Surface<'static> {
    // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る（呼ぶのは SCREEN を持っているときだけ）
    let buf = unsafe { &mut (&mut *CONSOLE_BUF.as_ptr())[..w * h] };
    Surface::new(buf, w, h)
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

pub(super) const MENU_TERMINAL: &str = "Terminal";

/// メニューから起動する。アプリは最初の端末につながる（出力はそこに出る）。終わるのは誰も待たない
fn launch(name: &str) {
    if name == MENU_TERMINAL {
        crate::gfx::console::open_terminal(None);
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
    if w.close_requested {
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
            s.windows.get(&id).is_none_or(|w| w.close_requested || !w.keys.is_empty() || !w.mouse.is_empty())
        };
        if timeout_ticks.is_some() {
            WaitQueue::wait_until_any(&[&INPUT_WAIT, crate::drivers::pit::tick_queue()], || has_event() || timed_out());
        } else {
            INPUT_WAIT.wait_until(has_event);
        }
    }
}
