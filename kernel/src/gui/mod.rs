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
//! 色: デスクトップは 32 ビットの画面（VESA の直接色）だけで、重ね合わせも 32 ビット（0x00RRGGBB）で
//! 行う。上端のバー・タイトルバー・メニュー・端末の中身は 32 ビットで描く（文字はなめらかなフォント。
//! gfx::fonts）。アプリのウィンドウ・背景のバッファは 8 ビット（パレットの色番号）で、重ね合わせるときに
//! 「色番号 → 32 ビットの色」の表で直す（独自のパレットを持つウィンドウはその表で）。
//! 全画面（320x200・モード 13h）は 8 ビットのまま、前面のウィンドウの独自のパレットを VGA にそのまま
//! 設定する。
//!
//! 入力: キーはフォーカスのあるウィンドウ（全画面では前面）に、マウスはその中の座標にして届ける。
//! ウィンドウの持ち主はそれぞれのキューから読む（`read_key` / `poll_mouse`）。

mod bar;
mod classic;
mod deco;
mod dock;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_font::FONT_8X8;
use vibe_gfx::canvas::{self, Canvas};
use vibe_gfx::{Surface, palette, theme};
use vibe_keyboard::{KeyCode, KeyEvent, Modifiers};

use crate::drivers::vga::{self, MAX_WIDTH};
use crate::sync::{BootCell, IrqMutex};
use crate::task::sched::TaskId;
use crate::task::wait::WaitQueue;
use core::sync::atomic::Ordering;
use crate::util::ring::Ring;

pub type WinId = u32;
pub const CONSOLE: WinId = 0;

/// タイトルバーの高さ・デスクトップの上端のバーの高さ（今風の画面）。16 色の画面は `classic` の値。
/// どちらの画面かで変わるので、使う所は `title_h()`・`bar_h()` で取る
const TITLE_H: usize = 24;
const BAR_H: usize = 24;

/// 16 色の画面（VGA のモード 12h）のデスクトップか（docs/plans/vga16.md）。デスクトップにするときに決まる
static CLASSIC: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn is_classic() -> bool {
    CLASSIC.load(Ordering::Relaxed)
}

/// タイトルバーの高さ
pub fn title_h() -> usize {
    if is_classic() { classic::TITLE_H } else { TITLE_H }
}

/// デスクトップの上端のバーの高さ
pub fn bar_h() -> usize {
    if is_classic() { classic::BAR_H } else { BAR_H }
}
/// 全画面のときのステータスバーの高さ（1 行）
pub const FS_BAR_H: usize = 8;
/// タイトルバーのボタン（[_]・[x]）の幅と、右端・ボタンどうしの間
const BUTTON_W: usize = 20;
const BUTTON_GAP: usize = 2;
/// デスクトップの端末のウィンドウの大きさ（文字数）。左下に置く
const TERMINAL_COLS: usize = 80;
const TERMINAL_ROWS: usize = 25;
/// 最初の端末（コンソール）の、全画面のときの一番大きな大きさ。バッファは起動直後から使うので静的な
/// 領域（8 ビット）に置く。画面がこれより大きければ、起動直後とパニックのときの全画面のコンソールは
/// 左上のこの大きさだけを使う。デスクトップでは最初の端末も 32 ビットのバッファをヒープから取る
const CONSOLE_MAX_W: usize = 640;
const CONSOLE_MAX_H: usize = 480;

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

/// デスクトップの背景の画像（0x00RRGGBB の w × h と並べ方）
pub struct Wallpaper {
    pub w: usize,
    pub h: usize,
    pub fit: vibe_gfx::wallpaper::Fit,
    pub pixels: Box<[u32]>,
    /// 16 色の画面では、16 色に誤差拡散した色番号（`pixels` は空）
    pub pixels8: Box<[u8]>,
}

/// ウィンドウの中身のバッファ
enum Pixels {
    /// 最初の端末の静的な領域（8 ビット。起動直後・全画面・パニックのとき）
    Static,
    /// 8 ビット（色番号）
    P8(Box<[u8]>),
    /// 32 ビット（0x00RRGGBB。デスクトップの端末）
    P32(Box<[u32]>),
}

/// ウィンドウの中身に描くためのもの。`P32` の表は色番号 → 32 ビットの色（色番号で描くときに使う）
pub enum WinPixels<'a> {
    P8(Surface<'a>),
    P32(Canvas<'a>, &'a [u32; 256]),
}

impl WinPixels<'_> {
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u8) {
        match self {
            WinPixels::P8(s) => s.fill_rect(x, y, w, h, color),
            WinPixels::P32(c, lut) => c.fill_rect(x, y, w, h, lut[color as usize]),
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u8) {
        self.fill_rect(x, y, w, 1, color);
    }

    pub fn scroll_up(&mut self, x: i32, y: i32, w: i32, h: i32, dy: i32, fill: u8) {
        match self {
            WinPixels::P8(s) => s.scroll_up(x, y, w, h, dy, fill),
            WinPixels::P32(c, lut) => c.scroll_up(x, y, w, h, dy, lut[fill as usize]),
        }
    }

    /// 8x8 のビットマップのフォントで文字列を描く
    pub fn draw_text8(&mut self, x: i32, y: i32, text: &str, fg: u8, bg: Option<u8>) {
        match self {
            WinPixels::P8(s) => {
                s.draw_text(&FONT_8X8, x, y, text, fg, bg);
            }
            WinPixels::P32(c, lut) => {
                c.draw_text(&FONT_8X8, x, y, text, lut[fg as usize], bg.map(|b| lut[b as usize]));
            }
        }
    }
}

/// ウィンドウの中身の 1 行
enum Row<'a> {
    P8(&'a [u8]),
    P32(&'a [u32]),
}

struct Window {
    w: usize,
    h: usize,
    pixels: Pixels,
    /// 中身の左上の画面上の位置（デスクトップのとき）
    x: i32,
    y: i32,
    title: String,
    /// 作ったプロセスのプログラムの名前（小文字、拡張子なし。ドックの項目と結び付ける。端末は "terminal"）
    app: String,
    owner: Option<TaskId>,
    /// [x] で持ち主のプロセスを止める（gfx_open の画面）
    kill_on_close: bool,
    closable: bool,
    /// タイトルバー（32 ビット、幅 × TITLE_H）
    titlebar: Vec<u32>,
    /// 16 色の画面のタイトルバー（色番号、幅 × classic::TITLE_H）
    title8: Vec<u8>,
    /// タイトルバー・上端のバーのボタンに出すアイコン（16x16）
    icon: Option<&'static [u32]>,
    /// 16 色の画面のアイコン（16x16 の色番号）
    icon16: Option<&'static [u8]>,
    /// 独自のパレット（全画面で前に出ている間だけ使う）
    palette: Option<Box<[[u8; 3]; 256]>>,
    /// 独自のパレットの色番号 → 32 ビットの色（デスクトップで重ね合わせるときに使う）
    lut: Option<Box<[u32; 256]>>,
    /// 独自のパレットの色番号 → モード 12h の 16 色の番号（`palette::map16`）
    map16: Option<Box<[u8; 256]>>,
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

static CONSOLE_BUF: BootCell<[u8; CONSOLE_MAX_W * CONSOLE_MAX_H]> =
    BootCell::new([0; CONSOLE_MAX_W * CONSOLE_MAX_H]);
/// 全画面のステータスバー（8 ビット。デスクトップの上端のバーは `Screen::bar`）
static BAR_BUF: BootCell<[u8; MAX_WIDTH * FS_BAR_H]> = BootCell::new([0; MAX_WIDTH * FS_BAR_H]);
/// 重ね合わせの 1 行（8 ビットと 32 ビット）。画面の幅が大きいとスタックに置けないので静的な領域に
static ROW8: BootCell<[u8; MAX_WIDTH]> = BootCell::new([0; MAX_WIDTH]);
static ROW32: BootCell<[u32; MAX_WIDTH]> = BootCell::new([0; MAX_WIDTH]);

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
    dirty_lo: usize,
    dirty_hi: usize,
    /// 書き換えた範囲の x（[dirty_x0, dirty_x1)。16 色の画面で VRAM に送るバイトを減らすのに使う）
    dirty_x0: usize,
    dirty_x1: usize,
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
    /// デスクトップの背景の画像（1 行ずつ並べる）
    wallpaper: Option<Box<Wallpaper>>,
    /// 標準のパレットの色番号 → 32 ビットの色
    std_lut: [u32; 256],
    /// 標準のパレットの色番号 → モード 12h の 16 色の番号
    std_map16: [u8; 256],
    /// デスクトップの端末の色番号 → 32 ビットの色（0〜15 を今風の配色に替えたもの。theme::TERMINAL）
    term_lut: [u32; 256],
    /// デスクトップの上端のバー（32 ビット、画面の幅 × BAR_H）
    bar: Vec<u32>,
    /// 16 色の画面の上端のバー（色番号、画面の幅 × classic::BAR_H）
    bar8: Vec<u8>,
    /// 16 色の画面: 32 ビットのウィンドウの色を 16 色にする表
    lut16: Option<Box<vibe_gfx::dither::Lut16>>,
    /// 16 色の画面: タイトルバーをドラッグしている間の枠の位置（中身の左上）
    outline: Option<(i32, i32)>,
    /// 角の丸み・影の表とカーソル（デスクトップにするときに作る）
    deco: Option<Box<deco::Deco>>,
    /// マウスが乗っているタイトルバーのボタン
    hover: Option<(WinId, Part)>,
    /// ドック（デスクトップで DOCK.CFG があるとき）
    dock: Option<Box<dock::Dock>>,
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
    capture: None,
    buttons: 0,
    placed: 0,
    status_text: String::new(),
    bar_dirty: false,
    menu: None,
    wallpaper: None,
    std_lut: [0; 256],
    std_map16: [0; 256],
    term_lut: [0; 256],
    bar: Vec::new(),
    bar8: Vec::new(),
    lut16: None,
    outline: None,
    deco: None,
    hover: None,
    dock: None,
    mouse_in: None,
});

/// キー・マウス・閉じる要求が届いたら起こされる
static INPUT_WAIT: WaitQueue = WaitQueue::new();

/// 起動直後（vga::init の後）に呼ぶ。まずはコンソールを画面いっぱい（一番上の行はステータスバー）に出す
pub fn init() {
    let mut s = SCREEN.lock();
    s.width = vga::width();
    s.height = vga::height();
    s.std_lut = palette::lut32(&palette::standard());
    s.std_map16 = palette::map16(&palette::standard());
    s.term_lut = s.std_lut;
    s.term_lut[..16].copy_from_slice(&theme::TERMINAL);
    s.console_w = s.width.min(CONSOLE_MAX_W);
    s.console_h = (s.height - FS_BAR_H).min(CONSOLE_MAX_H);
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
    let mut win = new_window(w, h, Pixels::Static, "Terminal 1", None, false, false);
    win.y = s.console_y as i32;
    win.terminal = true;
    win.tty = Some(CONSOLE);
    s.windows.insert(CONSOLE, Box::new(win));
    s.order.push(CONSOLE);
}

/// 32 ビットの画面ならデスクトップにする（スケジューラが動いた後に呼ぶ）。したら true
pub fn start_desktop() -> bool {
    if vga::bpp() == 4 {
        return start_classic();
    }
    if vga::bpp() != 32 || vga::width() < 640 || vga::height() < 400 {
        return false;
    }
    // ドックの項目（ファイルを読むのでロックの外で）
    let dock_items = dock::load();
    let mut s = SCREEN.lock();
    s.mode = Mode::Desktop;
    let (w, h) = (s.width, s.height);
    s.dock = dock::Dock::new(dock_items, w, h).map(Box::new);
    // 端末は作業領域（ドックの上）の左下
    let (cw, ch) = s.desktop_terminal_size();
    s.console_w = cw;
    s.console_h = ch;
    let y = (s.work_bottom() - ch - 8) as i32;
    s.console_y = y as usize;
    let bg = s.term_lut[palette::BLACK as usize];
    let width = s.width;
    s.bar = vec![0; width * bar_h()];
    s.deco = Some(Box::new(deco::Deco::new()));
    if let Some(c) = s.windows.get_mut(&CONSOLE) {
        c.icon = crate::gfx::icons::get("terminal");
        c.app = String::from("terminal");
        c.w = cw;
        c.h = ch;
        c.x = 8;
        c.y = y;
        c.closable = false;
        // デスクトップの端末は 32 ビット（なめらかな文字）
        c.pixels = Pixels::P32(vec![bg; cw * ch].into_boxed_slice());
    }
    s.refresh_titlebars();
    s.render_dock();
    s.refresh_dock();
    s.cursor_visible = true;
    s.bar_dirty = true;
    let h = s.height;
    s.mark_rows(0, h);
    s.present();
    true
}

/// 16 色の画面（モード 12h）のデスクトップにする（docs/plans/vga16.md。見た目は classic.rs）
fn start_classic() -> bool {
    CLASSIC.store(true, Ordering::Relaxed);
    let mut s = SCREEN.lock();
    s.mode = Mode::Desktop;
    s.start_classic();
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
    pixels: Pixels,
    title: &str,
    owner: Option<TaskId>,
    kill_on_close: bool,
    closable: bool,
) -> Window {
    Window {
        w,
        h,
        pixels,
        x: 0,
        y: 0,
        title: String::from(title),
        app: String::new(),
        owner,
        kill_on_close,
        closable,
        titlebar: Vec::new(),
        title8: Vec::new(),
        icon: None,
        icon16: None,
        palette: None,
        lut: None,
        map16: None,
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

    /// コンソールの静的な領域（全画面のときの端末の中身。パニックの画面もここに描く）
    pub fn surface(&mut self) -> Surface<'_> {
        static_surface(self.console_w, self.console_h)
    }

    /// コンソールの大きさ（ピクセル）
    pub fn console_size(&self) -> (usize, usize) {
        (self.console_w, self.console_h)
    }

    /// コンソールの行 [y0, y1) を書き換えたことを記録する
    pub fn mark_dirty(&mut self, y0: usize, y1: usize) {
        self.mark_window_dirty(CONSOLE, y0, y1);
    }

    /// 全画面のステータスバー（一番上の行）に描く。デスクトップの上端のバーは gui が描く（`set_status_text`）
    pub fn draw_status_bar(&mut self, draw: impl FnOnce(&mut Surface)) {
        let h = FS_BAR_H;
        let w = self.width;
        // SAFETY: BAR_BUF は SCREEN のロックの中でだけ触る
        let bar = unsafe { &mut (&mut *BAR_BUF.as_ptr())[..w * h] };
        draw(&mut Surface::new(bar, w, h));
        self.mark_rows(0, h);
    }

    /// ウィンドウを作って前に出す。`kill_on_close` なら [x] で持ち主を止め、そうでなければ閉じる要求のイベントを届ける。
    /// `tty` は属する端末（作ったプロセスの端末）、`app` はアイコンを選ぶアプリの名前（小文字、拡張子なし）。
    /// `rgb` なら中身は 32 ビット
    #[allow(clippy::too_many_arguments)]
    pub fn create_window(
        &mut self,
        w: usize,
        h: usize,
        title: &str,
        owner: Option<TaskId>,
        kill_on_close: bool,
        tty: Option<WinId>,
        app: &str,
        rgb: bool,
    ) -> WinId {
        let id = self.add_window_with(w, h, title, owner, kill_on_close, rgb);
        if let Some(win) = self.windows.get_mut(&id) {
            win.tty = tty;
            win.icon = crate::gfx::icons::for_app(app);
            win.icon16 = crate::gfx::icons::get16(app).or_else(|| crate::gfx::icons::get16("app"));
            win.app = String::from(app);
        }
        self.refresh_titlebars();
        // アプリの名前を付けた後でドックの点を直す
        self.refresh_dock();
        id
    }

    /// 端末のウィンドウを作って前に出す。全画面では画面いっぱい（一番上の行を除く）、デスクトップでは
    /// 64 桁 × 32 行
    pub fn create_terminal(&mut self, title: &str) -> WinId {
        let (w, h) = match self.mode {
            Mode::Fullscreen => (self.width, self.height - FS_BAR_H),
            Mode::Desktop => self.desktop_terminal_size(),
        };
        // 端末は今風の画面では 32 ビット（なめらかな文字）、16 色の画面と全画面では 8 ビット
        let rgb = self.mode == Mode::Desktop && !is_classic();
        let id = self.add_window_with(w, h, title, None, false, rgb);
        if let Some(win) = self.windows.get_mut(&id) {
            win.icon = crate::gfx::icons::get("terminal");
            win.icon16 = crate::gfx::icons::get16("terminal");
            win.app = String::from("terminal");
            win.terminal = true;
            win.tty = Some(id);
            if self.mode == Mode::Fullscreen {
                win.y = FS_BAR_H as i32;
            }
        }
        if is_classic() {
            // アイコンを付けた後でタイトルバーを描き直す
            self.refresh_titlebars();
        }
        self.refresh_dock();
        id
    }

    /// デスクトップの端末の中身の大きさ（ピクセル）。文字の枠は端末のフォント（無ければ 8x8）から
    fn desktop_terminal_size(&self) -> (usize, usize) {
        if is_classic() {
            // 8x16 の 80x25。幅は画面いっぱい（左右の枠は画面の外に出る）
            let f = classic::font();
            let (cw, ch) = (TERMINAL_COLS * f.width, TERMINAL_ROWS * f.height);
            return (cw.min(self.width), ch.min(self.work_bottom() - bar_h() - title_h() - 2 - 8));
        }
        let (fw, fh) = match crate::gfx::fonts::mono() {
            Some(f) => (f.cell_width as usize, f.line_height as usize),
            None => (FONT_8X8.width, FONT_8X8.height),
        };
        let (cw, ch) = (TERMINAL_COLS * fw, TERMINAL_ROWS * fh);
        (cw.min(self.width - 2), ch.min(self.work_bottom() - bar_h() - title_h() - 2 - 8))
    }

    /// `rgb` なら中身は 32 ビット、そうでなければ 8 ビット
    fn add_window_with(
        &mut self,
        w: usize,
        h: usize,
        title: &str,
        owner: Option<TaskId>,
        kill_on_close: bool,
        rgb: bool,
    ) -> WinId {
        self.next_id += 1; // 0 はコンソールなので 1 から
        let id = self.next_id;
        let pixels = if rgb {
            Pixels::P32(vec![self.term_lut[palette::BLACK as usize]; w * h].into_boxed_slice())
        } else {
            Pixels::P8(vec![palette::BLACK; w * h].into_boxed_slice())
        };
        let mut win = new_window(w, h, pixels, title, owner, kill_on_close, true);
        if self.mode == Mode::Desktop {
            // 少しずつずらして置く
            let n = (self.placed % 8) as i32;
            self.placed += 1;
            // 16 色の画面の端末は画面の幅いっぱいなので x = 0（左右の枠は画面の外）
            let max_x = (self.width as i32 - w as i32 - 1).max(if is_classic() { 0 } else { 1 });
            win.x = (120 + n * 24).min(max_x);
            win.y = ((bar_h() + title_h() + 1) as i32 + 24 + n * 20).min((self.work_bottom() as i32 - h as i32 - 1).max(30));
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
        }
        if self.capture == Some(id) {
            self.capture = None;
        }
        self.focus_changed();
    }

    /// 8 ビットのウィンドウの中身（32 ビットのウィンドウなら None。`window_pixels` を使う）
    pub fn window_surface(&mut self, id: WinId) -> Option<Surface<'_>> {
        match self.window_pixels(id)? {
            WinPixels::P8(s) => Some(s),
            WinPixels::P32(..) => None,
        }
    }

    /// ウィンドウの中身（8 ビットか 32 ビット）
    pub fn window_pixels(&mut self, id: WinId) -> Option<WinPixels<'_>> {
        let (std_lut, term_lut) = (&self.std_lut, &self.term_lut);
        let (cw, ch) = (self.console_w, self.console_h);
        match self.windows.get_mut(&id) {
            Some(win) => {
                let (w, h) = (win.w, win.h);
                // 端末は今風の 16 色、ほかは標準のパレット
                let lut = if win.terminal { term_lut } else { std_lut };
                Some(match &mut win.pixels {
                    Pixels::Static => WinPixels::P8(static_surface(cw, ch)),
                    Pixels::P8(b) => WinPixels::P8(Surface::new(b, w, h)),
                    Pixels::P32(b) => WinPixels::P32(Canvas::new(b, w, h), lut),
                })
            }
            // まだ登録していないコンソール（起動直後）
            None if id == CONSOLE => Some(WinPixels::P8(static_surface(cw, ch))),
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
        if bottom > 0 && self.mode == Mode::Desktop {
            self.mark_rect((top + y0 as i32).max(0) as usize, bottom as usize, left.max(0) as usize, right.max(0) as usize);
        } else if bottom > 0 {
            self.mark_rows((top + y0 as i32).max(0) as usize, bottom as usize);
        }
    }

    /// デスクトップの背景の画像を変える（None で既定の背景に戻す）。上端のバーも背景の上に描くので描き直す
    pub fn set_wallpaper(&mut self, wallpaper: Option<Wallpaper>) {
        self.wallpaper =
            wallpaper.filter(|w| w.pixels.len() == w.w * w.h || w.pixels8.len() == w.w * w.h).map(Box::new);
        self.bar_dirty = true;
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 背景の画像の大きさ（画面全体。上端のバーは背景の上に描く）
    pub fn wallpaper_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// 背景の `y` 行目: 既定の背景（紫の地に大きな円）か、背景の画像（真ん中に置いたときの周りは既定の背景）
    fn background_row(&self, y: usize, row: &mut [u32]) {
        let tiled = self.wallpaper.as_ref().is_some_and(|wp| wp.fit == vibe_gfx::wallpaper::Fit::Tile);
        if !tiled {
            vibe_gfx::backdrop::fill_row(row, y, self.height, theme::DESKTOP, &theme::DESKTOP_CIRCLES);
        }
        if let Some(wp) = &self.wallpaper {
            vibe_gfx::wallpaper::fill_row(row, y, &wp.pixels, wp.w, wp.h, self.height, wp.fit);
        }
    }

    /// ウィンドウの中身が 32 ビットか
    pub fn window_is_rgb(&self, id: WinId) -> bool {
        self.windows.get(&id).is_some_and(|w| matches!(w.pixels, Pixels::P32(_)))
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
        self.mark_rows((y - title_h() as i32).max(0) as usize, y.max(0) as usize);
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
        win.lut = Some(Box::new(palette::lut32(pal)));
        win.map16 = Some(Box::new(palette::map16(pal)));
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
        let (x, y) = (self.cursor_x.max(0) as usize, self.cursor_y.max(0) as usize);
        self.mark_rect(y, y + CURSOR_SHAPE.len().max(deco::CURSOR_H), x, x + deco::CURSOR_W);
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
        self.refresh_dock();
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
        if is_classic() {
            for (&id, win) in self.windows.iter_mut() {
                classic::render_titlebar(win, Some(id) == focused);
            }
            return;
        }
        let hover = self.hover;
        let Some(deco) = self.deco.as_deref() else { return };
        for (&id, win) in self.windows.iter_mut() {
            let part = hover.filter(|h| h.0 == id).map(|h| h.1);
            render_titlebar(win, Some(id) == focused, part, deco);
        }
    }

    /// (x, y) にある一番手前のウィンドウとその部分
    fn hit(&self, x: i32, y: i32) -> Option<(WinId, Part)> {
        // ドックはウィンドウより手前
        if self.dock_contains(x, y) {
            return None;
        }
        for &id in self.order.iter().rev() {
            let w = &self.windows[&id];
            if w.minimized {
                continue;
            }
            let (left, right) = (w.x - 1, w.x + w.w as i32);
            let (top, bottom) = (w.y - title_h() as i32 - 1, w.y + w.h as i32);
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

    fn move_window(&mut self, id: WinId, x: i32, y: i32) {
        let (sw, sh) = (self.width as i32, self.height as i32);
        let Some(win) = self.windows.get_mut(&id) else { return };
        // 影の分も描き直す
        let m = deco::SHADOW_MARGIN;
        let old = (win.y - title_h() as i32 - 1 - m, win.y + win.h as i32 + 1 + m);
        // タイトルバーが上端のバーの下に、少なくとも一部が画面に残るようにする
        win.x = x.clamp(-(win.w as i32) + 40, sw - 40);
        win.y = y.clamp((bar_h() + title_h() + 1) as i32, sh - 4);
        let new = (win.y - title_h() as i32 - 1 - m, win.y + win.h as i32 + 1 + m);
        for (a, b) in [old, new] {
            self.mark_rows(a.max(0) as usize, b.max(0) as usize);
        }
    }

    /// 入力スレッドから: マウスが (x, y)（画面の座標）にあり、ボタンが `buttons`
    fn on_mouse(&mut self, x: i32, y: i32, buttons: u8) -> Option<MouseAction> {
        self.set_cursor(x, y);
        if self.mode == Mode::Desktop {
            self.update_hover(x, y, buttons);
            let free = self.drag.is_none() && self.menu.is_none() && self.capture.is_none();
            self.dock_hover(free.then_some((x, y)));
        }
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
                if is_classic() {
                    self.move_outline(x - dx, y - dy);
                } else {
                    self.move_window(id, x - dx, y - dy);
                }
                return None;
            }
            if is_classic() {
                self.finish_outline();
            }
            self.drag = None;
        } else if left && !was_left && y < bar_h() as i32 && self.capture.is_none() {
            match self.bar_hit(x) {
                Some(bar::BarHit::Menu) => return Some(MouseAction::OpenMenu),
                // フォーカスのあるウィンドウのボタンなら最小化、ほかは前に出す（最小化していれば戻す）
                Some(bar::BarHit::Window(id)) if self.focused() == Some(id) => self.minimize(id),
                Some(bar::BarHit::Window(id)) => self.raise(id),
                None => {}
            }
            return None;
        } else if left && !was_left && self.capture.is_none() && self.dock_contains(x, y) {
            return self.dock_click(x, y);
        } else if left && !was_left {
            match self.hit(x, y) {
                Some((id, Part::Title)) => {
                    self.raise(id);
                    let w = &self.windows[&id];
                    let (wx, wy) = (w.x, w.y);
                    self.drag = Some((id, x - wx, y - wy));
                    if is_classic() {
                        // 16 色の画面では枠だけを動かす（離したときにウィンドウを動かす）
                        self.outline = None;
                        self.move_outline(wx, wy);
                    }
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

    /// マウスが乗っているタイトルバーのボタンが変わったら、そのタイトルバーを描き直す
    fn update_hover(&mut self, x: i32, y: i32, buttons: u8) {
        if is_classic() {
            return; // 16 色の画面のボタンは乗せても変わらない
        }
        let hover = match self.hit(x, y) {
            Some((id, p @ (Part::Close | Part::Minimize))) if buttons == 0 && self.drag.is_none() && self.menu.is_none() => {
                Some((id, p))
            }
            _ => None,
        };
        if hover == self.hover {
            return;
        }
        let old = core::mem::replace(&mut self.hover, hover);
        let focused = self.focused();
        let Some(deco) = self.deco.as_deref() else { return };
        let mut rows = [None; 2];
        for (slot, (id, _)) in rows.iter_mut().zip([old, hover].into_iter().flatten()) {
            let Some(w) = self.windows.get_mut(&id) else { continue };
            let part = hover.filter(|h| h.0 == id).map(|h| h.1);
            render_titlebar(w, focused == Some(id), part, deco);
            *slot = Some(((w.y - title_h() as i32).max(0) as usize, w.y.max(0) as usize));
        }
        for (top, bottom) in rows.into_iter().flatten() {
            self.mark_rows(top, bottom);
        }
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
        self.console_w = self.width.min(CONSOLE_MAX_W);
        self.console_h = self.height.min(CONSOLE_MAX_H);
        self.console_y = 0;
        self.order.retain(|&w| w != CONSOLE);
        self.order.push(CONSOLE);
        self.cursor_visible = false;
        let (cw, ch) = (self.console_w, self.console_h);
        if let Some(c) = self.windows.get_mut(&CONSOLE) {
            c.w = cw;
            c.h = ch;
            c.x = 0;
            c.y = 0;
            c.pixels = Pixels::Static;
        }
        vga::set_palette(&palette::standard());
        let h = self.height;
        self.mark_rows(0, h);
    }

    /// 書き換えた行を重ね合わせて VRAM へ送る
    pub fn present(&mut self) {
        if self.bar_dirty && self.mode == Mode::Desktop {
            if is_classic() {
                self.render_bar_classic();
            } else {
                self.render_bar();
            }
        }
        if self.dirty_lo >= self.dirty_hi {
            return;
        }
        // SAFETY: ROW8・ROW32 は SCREEN のロックの中（ここ）でだけ触る
        let row8 = unsafe { &mut (&mut *ROW8.as_ptr())[..self.width] };
        let row32 = unsafe { &mut (&mut *ROW32.as_ptr())[..self.width] };
        let focused = self.focused();
        if self.mode == Mode::Desktop && is_classic() {
            // 16 色の画面: 色番号で重ね、書き換えた x の範囲だけを 4 枚の面に送る
            let (x0, x1) = (self.dirty_x0, self.dirty_x1);
            for y in self.dirty_lo..self.dirty_hi {
                self.compose_row_classic(y, row8, focused);
                vga::write_row4_span(y, row8, x0, x1);
            }
            self.dirty_lo = 0;
            self.dirty_hi = 0;
            return;
        }
        for y in self.dirty_lo..self.dirty_hi {
            if self.mode == Mode::Desktop {
                self.compose_desktop_row(y, row32, focused);
                if self.cursor_visible {
                    self.draw_cursor_row32(y, row32);
                }
                vga::write_row32(y, row32);
                continue;
            }
            self.compose_row(y, row8);
            if self.cursor_visible {
                self.draw_cursor_row(y, row8);
            }
            if vga::bpp() == 32 {
                // 32 ビットの画面の全画面（パニックのとき）: 前面のウィンドウのパレットで直す
                let lut = self.windows.get(&self.foreground()).and_then(|w| w.lut.as_deref()).unwrap_or(&self.std_lut);
                convert_row(row32, row8, lut);
                vga::write_row32(y, row32);
            } else if vga::bpp() == 4 {
                // モード 12h: 前面のウィンドウのパレットの色を 16 色の近い色に直す
                let map = self.windows.get(&self.foreground()).and_then(|w| w.map16.as_deref()).unwrap_or(&self.std_map16);
                for c in row8.iter_mut() {
                    *c = map[*c as usize];
                }
                vga::write_row4(y, row8);
            } else {
                vga::write_row(y, row8);
            }
        }
        self.dirty_lo = 0;
        self.dirty_hi = 0;
    }

    fn client_row(&self, id: WinId, y: usize) -> Row<'_> {
        let w = &self.windows[&id];
        match &w.pixels {
            Pixels::P8(b) => Row::P8(&b[y * w.w..(y + 1) * w.w]),
            Pixels::P32(b) => Row::P32(&b[y * w.w..(y + 1) * w.w]),
            Pixels::Static => {
                // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る（今ロックを持っている）
                let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
                Row::P8(&buf[y * self.console_w..(y + 1) * self.console_w])
            }
        }
    }

    /// 全画面の 1 行（8 ビット）
    fn compose_row(&self, y: usize, row: &mut [u8]) {
        row.fill(palette::BLACK);
        let fg = self.foreground();
        let top = match self.windows.get(&fg) {
            Some(win) => win.y.max(0) as usize,
            None => self.console_y,
        };
        if y < top {
            // 端末の上の行はステータスバー
            // SAFETY: BAR_BUF は SCREEN のロックの中でだけ触る（今ロックを持っている）
            let bar = unsafe { &*BAR_BUF.as_ptr() };
            if y < FS_BAR_H {
                row.copy_from_slice(&bar[y * self.width..(y + 1) * self.width]);
            }
        } else if let Some(win) = self.windows.get(&fg) {
            if y - top < win.h {
                let n = win.w.min(row.len());
                match self.client_row(fg, y - top) {
                    Row::P8(r) => row[..n].copy_from_slice(&r[..n]),
                    // 32 ビットのウィンドウは標準のパレットの近い色に
                    Row::P32(r) => row[..n].iter_mut().zip(r).for_each(|(d, &c)| *d = canvas::to_palette8(c)),
                }
            }
        } else if y - top < self.console_h {
            // まだウィンドウを登録していない（起動直後）
            // SAFETY: 同上
            let buf = unsafe { &*CONSOLE_BUF.as_ptr() };
            let (r, w) = (y - top, self.console_w.min(row.len()));
            row[..w].copy_from_slice(&buf[r * self.console_w..r * self.console_w + w]);
        }
    }

    /// デスクトップの 1 行（32 ビット）
    fn compose_desktop_row(&self, y: usize, row: &mut [u32], focused: Option<WinId>) {
        if y < bar_h() {
            row.copy_from_slice(&self.bar[y * self.width..(y + 1) * self.width]);
            return;
        }
        self.background_row(y, row);
        for &id in &self.order {
            self.draw_window_row(id, y as i32, row, focused == Some(id));
        }
        self.draw_dock_row(y, row);
        self.draw_menu_row(y, row);
    }

    /// ウィンドウ（枠とタイトルバーを含む角の丸い矩形と影）の `y` 行目を重ねる
    fn draw_window_row(&self, id: WinId, y: i32, row: &mut [u32], focused: bool) {
        let w = &self.windows[&id];
        let Some(deco) = &self.deco else { return };
        if w.minimized {
            return;
        }
        // タイトルバーと中身の矩形（縁は描かない。当たり判定と描き直す行は 1 ピクセル広い矩形のまま）
        let rect = (w.x, w.y - title_h() as i32, w.w as i32, w.h as i32 + title_h() as i32);
        let m = deco::SHADOW_MARGIN;
        if y < rect.1 - m || y >= rect.1 + rect.3 + m {
            return;
        }
        let fill = |x0: i32, span: &mut [u32]| {
            // 中身（タイトルバーか、ウィンドウの中身）の行と、その左端の x
            let src = if y >= w.y - title_h() as i32 && y < w.y {
                let r = (y - (w.y - title_h() as i32)) as usize;
                Row::P32(&w.titlebar[r * w.w..(r + 1) * w.w])
            } else if y >= w.y && y < w.y + w.h as i32 {
                self.client_row(id, (y - w.y) as usize)
            } else {
                return;
            };
            let (s0, s1) = ((x0 - w.x).max(0), (x0 + span.len() as i32 - w.x).min(w.w as i32));
            if s0 >= s1 {
                return;
            }
            let d = &mut span[(w.x + s0 - x0) as usize..(w.x + s1 - x0) as usize];
            let (s0, s1) = (s0 as usize, s1 as usize);
            match src {
                Row::P8(r) => convert_row(d, &r[s0..s1], w.lut.as_deref().unwrap_or(&self.std_lut)),
                Row::P32(r) => d.copy_from_slice(&r[s0..s1]),
            }
        };
        deco::panel_row(row, y, rect, &deco.corner, &deco.shadows(focused), fill);
    }

    fn draw_cursor_row32(&self, y: usize, row: &mut [u32]) {
        if let Some(d) = &self.deco {
            deco::cursor_row(row, y as i32, self.cursor_x, self.cursor_y, &d.cursor);
            return;
        }
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
                row[x as usize] = self.std_lut[color as usize];
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

/// 8 ビットの色番号の並びを表 `lut` で 32 ビットの色に直す
fn convert_row(dst: &mut [u32], src: &[u8], lut: &[u32; 256]) {
    for (d, &c) in dst.iter_mut().zip(src) {
        *d = lut[c as usize];
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
        self.closable.then(|| self.w as i32 - (BUTTON_W + BUTTON_GAP) as i32)
    }

    /// タイトルバーの [_] の左端（[x] の左、[x] が無ければ右端）
    fn minimize_box_x(&self) -> i32 {
        let x = self.w as i32 - (BUTTON_W + BUTTON_GAP) as i32;
        if self.closable { x - (BUTTON_W + BUTTON_GAP) as i32 } else { x }
    }
}

/// コンソールの静的な領域（`w` × `h`。CONSOLE_MAX_W × CONSOLE_MAX_H まで）
fn static_surface(w: usize, h: usize) -> Surface<'static> {
    // SAFETY: CONSOLE_BUF は SCREEN のロックの中でだけ触る（呼ぶのは SCREEN を持っているときだけ）
    let buf = unsafe { &mut (&mut *CONSOLE_BUF.as_ptr())[..w * h] };
    Surface::new(buf, w, h)
}

/// UI の文字の種類
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Face {
    Ui,
    Bold,
    /// 固定幅（バーの状態の表示: 数が変わっても位置が動かないように）
    Mono,
}

fn face_font(face: Face) -> Option<&'static vibe_font::VFont<'static>> {
    match face {
        Face::Ui => crate::gfx::fonts::ui(),
        Face::Bold => crate::gfx::fonts::ui_bold(),
        Face::Mono => crate::gfx::fonts::mono(),
    }
}

/// UI の文字の幅（なめらかなフォント。無ければ 8x8）
pub(crate) fn label_width(face: Face, text: &str) -> i32 {
    if is_classic() {
        return classic::text_width(text);
    }
    match face_font(face) {
        Some(f) => f.measure(text),
        None => (text.chars().count() * FONT_8X8.width) as i32,
    }
}

/// 幅 `max` に入る先頭の文字数
pub(crate) fn label_fit(face: Face, text: &str, max: i32) -> usize {
    if is_classic() {
        return classic::text_fit(text, max);
    }
    match face_font(face) {
        Some(f) => f.fit(text, max),
        None => (max.max(0) as usize / FONT_8X8.width).min(text.chars().count()),
    }
}

/// UI の文字を 1 行描く。`top`〜`top + h` の帯の縦の真ん中に置く。描いた幅を返す
pub(crate) fn draw_label(c: &mut Canvas, face: Face, x: i32, top: i32, h: i32, text: &str, color: u32) -> i32 {
    match face_font(face) {
        Some(f) => c.draw_vtext(f, x, top + (h - f.line_height as i32) / 2, text, color),
        None => c.draw_text(&FONT_8X8, x, top + (h - FONT_8X8.height as i32) / 2, text, color, None),
    }
}

/// タイトルバーを描く: 左にアイコンと題名（太字）、右端に [_] と、閉じられるなら [x]（薄い地の丸に線の記号。
/// マウスが乗ると丸が濃くなり、[x] は赤に白）。地は中身と同じで区切りの線は無い。フォーカスが無ければ
/// 地はそのままで、題名・記号を薄い色に、アイコンを半分の濃さにする。端末は中身と同じ暗い地
fn render_titlebar(w: &mut Window, focused: bool, hover: Option<Part>, deco: &deco::Deco) {
    let close_x = w.close_box_x();
    let min_x = w.minimize_box_x();
    w.titlebar.resize(w.w * title_h(), 0);
    let mut c = Canvas::new(&mut w.titlebar, w.w, title_h());
    let h = title_h() as i32;
    let (bg, fg, fg_dim, button_bg, button_hover) = if w.terminal {
        (theme::TERMINAL[0], theme::DARK_TEXT, theme::DARK_TEXT_DIM, theme::DARK_BUTTON, theme::DARK_HOVER)
    } else {
        (theme::FACE, theme::TEXT, theme::TEXT_DIM, theme::SUBTLE, theme::HOVER)
    };
    let ink = if focused { fg } else { fg_dim };
    c.clear(bg);
    let mut tx = 8;
    if let Some(icon) = w.icon {
        let s = crate::gfx::icons::SIZE as i32;
        c.blit_alpha_faded(6, (h - s) / 2, icon, s as usize, if focused { 255 } else { 128 });
        tx = 6 + s + 6;
    }
    let n = label_fit(Face::Bold, &w.title, min_x - tx - 4);
    let title: String = w.title.chars().take(n).collect();
    draw_label(&mut c, Face::Bold, tx, 0, h, &title, ink);
    // ボタン: 押せる所（幅 BUTTON_W）の真ん中に直径 BUTTON_W の丸と、その真ん中に 8x8 の記号
    let d = BUTTON_W as i32;
    let by = (h - d) / 2;
    let button = |c: &mut Canvas, x: i32, part: Part| -> canvas::Color {
        let (fill, col) = match hover {
            Some(p) if p == part && part == Part::Close => (theme::CLOSE_HOVER, canvas::rgb(255, 255, 255)),
            Some(p) if p == part => (button_hover, ink),
            _ => (button_bg, ink),
        };
        c.fill_round_rect(x, by, d, d, &deco.button, fill, 255);
        col
    };
    let (gx, gy) = ((d - 8) / 2, by + (d - 8) / 2);
    let col = button(&mut c, min_x, Part::Minimize);
    c.fill_rect(min_x + gx, gy + 4, 8, 1, col);
    c.fill_rect_alpha(min_x + gx, gy + 5, 8, 1, col, 90);
    if let Some(x) = close_x {
        let col = button(&mut c, x, Part::Close);
        // ×: 2 本の斜めの線（隣のピクセルを薄く重ねて太さとなめらかさを出す）
        for d in 0..8 {
            for (px, py) in [(x + gx + d, gy + d), (x + gx + 7 - d, gy + d)] {
                c.blend_pixel(px, py, col, 255);
                c.blend_pixel(px + 1, py, col, 110);
                c.blend_pixel(px - 1, py, col, 50);
            }
        }
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

pub(super) const MENU_TERMINAL: &str = "Terminal";

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
