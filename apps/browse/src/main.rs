//! browse: 簡易ウェブブラウザ（ウィンドウ。docs/plans/browser.md）
//!
//! browse [URL]   URL は `http://…`・`file:///…`・`/` で始まるパス・カレントディレクトリからの相対パス。
//! 無ければ始めのページ。
//!
//! HTML を取ってきて、文字・見出し・リスト・リンク・表（行ごと）・画像として並べる（中身は `vibe-web`）。
//! CSS・JavaScript・HTTPS は扱わない。
//!
//! - 上: 戻る・進む・読み直しのボタンとアドレス欄（Enter で移動）。下: 乗せたリンクの行き先や読み込みの様子
//! - リンクを押すと移動。`#…` だけ違うリンクは同じページの中でその場所へ動く
//! - キー: ↑↓・PageUp/PageDown・Space・Home/End でスクロール、Alt+←/→ で戻る・進む、F5 で読み直し、
//!   Ctrl+L でアドレス欄へ、Esc で読み込みを止める
//! - 読み込んだら `browse: loaded URL (N lines) "タイトル"` を標準出力に出す（テストで待つ）

#![no_std]
#![no_main]

extern crate alloc;

mod fetch;
mod pages;
mod view;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use vibe_rt::abi::{KeyEvent, key};
use vibe_rt::gfx::{Event, Window32 as Window};
use vibe_rt::gfx_lib::theme;
use vibe_rt::{println, sys};
use vibe_ui::{Action, Button, Id, Input, Rect, ScrollBar, TextField, Ui};
use vibe_web::charset;
use vibe_web::doc::{self, Document};
use vibe_web::layout::{self, Layout};
use vibe_web::url::{Scheme, Url, UrlError};

use crate::view::Fonts;

vibe_rt::entry!(main);

const TOOLBAR_H: i32 = 36;
const STATUS_H: i32 = 20;
const SCROLLBAR_W: i32 = 12;
const BUTTON_W: i32 = 30;
const GAP: i32 = 4;
/// ↑↓で動く量
const STEP: i32 = 40;
/// 履歴の上限
const HISTORY_MAX: usize = 50;
/// ウィンドウの中身の大きさの上限（カーネルの win_open の上限）
const MAX_W: i32 = 640;
const MAX_H: i32 = 480;

/// キーの番号（セット 1 のスキャンコード）
const KEY_L: u8 = 0x26;
const KEY_F5: u8 = 0x3F;

struct Page {
    url: Url,
    /// リンクを解決する元（`<base href>` があればそれ）
    base: Url,
    doc: Document,
    layout: Layout,
}

struct Visit {
    url: Url,
    scroll: i32,
    /// 始めのページ（アプリが作るページ。取りに行かない）
    start: bool,
}

/// 移動のしかた
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    /// 新しく開く（履歴に足す）
    New,
    /// 履歴の中を動く（スクロールの位置を戻す）
    History,
    Reload,
}

struct App {
    ui: Ui,
    back: Id,
    forward: Id,
    reload: Id,
    address: Id,
    fonts: Fonts,
    w: i32,
    h: i32,
    page: Option<Page>,
    history: Vec<Visit>,
    /// 今いる履歴の位置
    current: usize,
    sb: ScrollBar,
    /// マウスが乗っているリンク
    hover: Option<u32>,
    /// 押したリンク（同じリンクの上で離したら移動）
    pressed: Option<u32>,
    buttons: u32,
    status: String,
}

fn main() -> i32 {
    if !vibe_rt::gfx::is_desktop() {
        println!("browse: use it on the desktop (not in the 320x200 mode)");
        return 1;
    }
    let arg = vibe_rt::args().nth(1).map(String::from);
    let start = match arg.as_deref().map(parse_target) {
        Some(Ok(u)) => Some(u),
        Some(Err(e)) => {
            println!("browse: {}", e.message());
            return 2;
        }
        None => None,
    };
    vibe_ui::set_fonts(vibe_rt::gfx::font(vibe_rt::abi::FONT_UI), vibe_rt::gfx::font(vibe_rt::abi::FONT_BOLD));
    let info = sys::sys_info();
    // 上端のバー（24）・ドック（66）・タイトルバー（24）と少しの余白を除いた大きさ
    let w = (info.screen_width as i32 - 16).clamp(320, MAX_W);
    let h = (info.screen_height as i32 - 24 - 66 - 24 - 16).clamp(240, MAX_H);
    let Some(mut win) = Window::open(w as usize, h as usize, "Browse") else {
        println!("browse: cannot open a window");
        return 1;
    };
    let mut app = App::new(w, h);
    println!("browse: started");
    match start {
        Some(u) => app.go(&mut win, u, Nav::New),
        None => app.show_start(&mut win, Nav::New),
    }
    let mut dirty = true;
    loop {
        if dirty {
            app.draw(&mut win);
        }
        dirty = match win.event(None) {
            Some(Event::Close) | None => break,
            Some(Event::Key(k)) => app.key(&mut win, k),
            Some(Event::Mouse { x, y, buttons }) => app.mouse(&mut win, x, y, buttons),
        };
    }
    drop(win);
    println!("browse: closed");
    0
}

/// 引数・アドレス欄の文字を URL にする。形式の名前が無ければ、`/` で始まるならファイル、
/// `.` を含む名前で始まるなら `http://` を付け、ほかはカレントディレクトリからのファイル
fn parse_target(s: &str) -> Result<Url, UrlError> {
    let s = s.trim();
    if s.contains(':') || s.starts_with('/') {
        return Url::parse(s);
    }
    let head = s.split('/').next().unwrap_or("");
    // 8.3 のファイルの名前（拡張子が大文字 3 文字まで）でなければホストの名前とみなす
    let file_like =
        head.rsplit_once('.').is_some_and(|(_, ext)| ext.len() <= 3 && ext.chars().all(|c| c.is_ascii_uppercase()));
    if head.contains('.') && !head.contains("..") && !file_like {
        return Url::parse(&format!("http://{s}"));
    }
    let cwd = sys::getcwd().unwrap_or_else(|_| String::from("/"));
    let dir = if cwd.ends_with('/') { cwd } else { format!("{cwd}/") };
    Url::parse(&dir)?.join(s)
}

impl App {
    fn new(w: i32, h: i32) -> App {
        let mut ui = Ui::new();
        let y = (TOOLBAR_H - 26) / 2;
        let back = ui.add(Rect::new(GAP + 2, y, BUTTON_W, 26), Button::new("<"));
        let forward = ui.add(Rect::new(GAP + 2 + BUTTON_W + GAP, y, BUTTON_W, 26), Button::new(">"));
        let reload_x = GAP + 2 + 2 * (BUTTON_W + GAP);
        let reload = ui.add(Rect::new(reload_x, y, 58, 26), Button::new("Reload"));
        let ax = reload_x + 58 + GAP + 2;
        let address = ui.add(Rect::new(ax, y, w - ax - GAP - 2, 26), TextField::new(""));
        ui.clear_focus();
        App {
            ui,
            back,
            forward,
            reload,
            address,
            fonts: Fonts::load(),
            w,
            h,
            page: None,
            history: Vec::new(),
            current: 0,
            sb: ScrollBar::new(),
            hover: None,
            pressed: None,
            buttons: 0,
            status: String::new(),
        }
    }

    /// 本文の枠
    fn content(&self) -> Rect {
        Rect::new(0, TOOLBAR_H, self.w - SCROLLBAR_W, self.h - TOOLBAR_H - STATUS_H)
    }

    fn track(&self) -> Rect {
        let c = self.content();
        Rect::new(c.x + c.w, c.y, SCROLLBAR_W, c.h)
    }

    fn draw(&mut self, win: &mut Window) {
        self.update_buttons();
        let content = self.content();
        let track = self.track();
        let (w, h) = (self.w, self.h);
        let mut c = win.canvas();
        match &self.page {
            Some(p) => view::draw_page(&mut c, &self.fonts, &p.layout, content, self.sb.pos, self.hover, &|_| None),
            None => c.fill_rect(content.x, content.y, content.w, content.h, view::PAGE_BG),
        }
        c.fill_rect(track.x, track.y, track.w, track.h, view::PAGE_BG);
        self.sb.draw(&mut c, track);
        // 上の道具の並びと下の行（本文の上に描く。はみ出した文字を隠す）
        c.fill_rect(0, 0, w, TOOLBAR_H, theme::SUBTLE);
        c.hline(0, TOOLBAR_H - 1, w, theme::HOVER);
        let sy = h - STATUS_H;
        c.fill_rect(0, sy, w, STATUS_H, theme::SUBTLE);
        c.hline(0, sy, w, theme::HOVER);
        let status = match (self.hover, &self.page) {
            (Some(l), Some(p)) => p.link_text(l),
            _ => self.status.clone(),
        };
        vibe_rt::gfx::draw_text(&mut c, vibe_rt::abi::FONT_UI, 6, sy + 2, &status, theme::TEXT_DIM);
        self.ui.draw(&mut c);
        win.present();
    }

    fn update_buttons(&mut self) {
        let back = self.current > 0;
        let forward = self.current + 1 < self.history.len();
        for (id, on) in [(self.back, back), (self.forward, forward), (self.reload, self.page.is_some())] {
            if let vibe_ui::Widget::Button(b) = self.ui.widget_mut(id) {
                b.enabled = on;
            }
        }
    }

    /// キー。描き直しが要れば true
    fn key(&mut self, win: &mut Window, k: KeyEvent) -> bool {
        if k.pressed() {
            match k.key() {
                key::LEFT if k.alt() => return self.back(win),
                key::RIGHT if k.alt() => return self.forward(win),
                KEY_F5 => return self.reload(win),
                KEY_L if k.ctrl() => {
                    self.ui.set_focus(self.address);
                    if let vibe_ui::Widget::TextField(t) = self.ui.widget_mut(self.address) {
                        t.select_all();
                    }
                    return true;
                }
                _ => {}
            }
        }
        match self.ui.handle(Input::Key(k), sys::ticks()) {
            Some(Action::Key(k)) if k.pressed() => self.scroll_key(k),
            Some(a) => self.act(win, a),
            None => true,
        }
    }

    fn scroll_key(&mut self, k: KeyEvent) -> bool {
        let page = self.content().h - STEP;
        let pos = self.sb.pos;
        let to = match k.key() {
            key::UP => pos - STEP,
            key::DOWN => pos + STEP,
            key::PAGE_UP => pos - page,
            key::PAGE_DOWN | key::SPACE if !k.shift() => pos + page,
            key::SPACE => pos - page,
            key::HOME => 0,
            key::END => self.sb.max_pos(),
            _ => return false,
        };
        self.scroll_to(to)
    }

    fn scroll_to(&mut self, y: i32) -> bool {
        let changed = self.sb.set_pos(y);
        if changed {
            self.hover = None;
        }
        changed
    }

    fn act(&mut self, win: &mut Window, a: Action) -> bool {
        match a {
            Action::Clicked(id) if id == self.back => self.back(win),
            Action::Clicked(id) if id == self.forward => self.forward(win),
            Action::Clicked(id) if id == self.reload => self.reload(win),
            Action::Submitted(id) if id == self.address => {
                let text = String::from(self.ui.text(self.address));
                match parse_target(&text) {
                    Ok(u) => {
                        self.ui.clear_focus();
                        self.go(win, u, Nav::New);
                    }
                    Err(e) => {
                        self.status = format!("{text}: {}", e.message());
                        println!("browse: {}", self.status);
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn mouse(&mut self, win: &mut Window, x: i32, y: i32, buttons: u32) -> bool {
        let press = buttons & 1 != 0 && self.buttons & 1 == 0;
        let release = buttons & 1 == 0 && self.buttons & 1 != 0;
        self.buttons = buttons;
        let mut dirty = false;
        // スクロールバー
        let track = self.track();
        if self.sb.dragging() || (press && track.contains(x, y)) {
            return self.sb.mouse(track, x, y, press, buttons & 1 != 0) || press;
        }
        // 本文のリンク
        let content = self.content();
        let link = if content.contains(x, y) {
            self.page.as_ref().and_then(|p| p.layout.link_at(x - content.x, y - content.y + self.sb.pos))
        } else {
            None
        };
        if link != self.hover {
            self.hover = link;
            dirty = true;
        }
        if press && content.contains(x, y) {
            self.ui.clear_focus();
            self.pressed = link;
            dirty = true;
        }
        if release
            && let Some(l) = self.pressed.take()
            && Some(l) == link
        {
            self.follow(win, l);
            return true;
        }
        let before = (self.ui.hover(), self.ui.focus());
        if let Some(a) = self.ui.handle(Input::Mouse { x, y, buttons }, sys::ticks()) {
            return self.act(win, a);
        }
        dirty || before != (self.ui.hover(), self.ui.focus()) || press || release
    }

    /// リンク `l` をたどる
    fn follow(&mut self, win: &mut Window, l: u32) {
        let Some(p) = &self.page else { return };
        let href = p.doc.links[l as usize].clone();
        match p.base.join(&href) {
            Ok(u) => {
                println!("browse: link {u}");
                self.go(win, u, Nav::New);
            }
            Err(e) => {
                self.hover = None;
                self.status = format!("{}: {}", href.trim(), e.message());
                println!("browse: {}", self.status);
            }
        }
    }

    fn back(&mut self, win: &mut Window) -> bool {
        if self.current > 0 {
            self.history[self.current].scroll = self.sb.pos;
            self.current -= 1;
            let u = self.history[self.current].url.clone();
            self.go(win, u, Nav::History);
        }
        true
    }

    fn forward(&mut self, win: &mut Window) -> bool {
        if self.current + 1 < self.history.len() {
            self.history[self.current].scroll = self.sb.pos;
            self.current += 1;
            let u = self.history[self.current].url.clone();
            self.go(win, u, Nav::History);
        }
        true
    }

    fn reload(&mut self, win: &mut Window) -> bool {
        if let Some(p) = &self.page {
            let u = p.url.clone();
            self.go(win, u, Nav::Reload);
        }
        true
    }

    /// 履歴に足す（今より先の分は捨てる）
    fn push_history(&mut self, url: &Url) {
        if let Some(v) = self.history.get_mut(self.current) {
            v.scroll = self.sb.pos;
        }
        if !self.history.is_empty() {
            self.history.truncate(self.current + 1);
        }
        self.history.push(Visit { url: url.clone(), scroll: 0, start: false });
        if self.history.len() > HISTORY_MAX {
            self.history.remove(0);
        }
        self.current = self.history.len() - 1;
    }

    /// `url` へ移動する
    fn go(&mut self, win: &mut Window, url: Url, nav: Nav) {
        self.hover = None;
        self.pressed = None;
        if nav != Nav::New && self.history.get(self.current).is_some_and(|v| v.start) {
            self.show_start(win, nav);
            return;
        }
        // 同じ文書の中の印へ
        if nav != Nav::Reload && url.fragment.is_some() && self.page.as_ref().is_some_and(|p| p.url.same_document(&url))
        {
            if nav == Nav::New {
                self.push_history(&url);
            }
            if let Some(p) = &mut self.page {
                p.url = url.clone();
            }
            let y = self.target_scroll(&url, nav);
            self.sb.set_pos(y);
            self.ui.set_text(self.address, &url.to_string());
            println!("browse: moved to {url}");
            return;
        }
        self.status = format!("Opening {url} ...");
        self.draw(win);
        let mut stop = false;
        let result = {
            let mut progress = |msg: &str| {
                // Esc で止める（ほかのイベントは捨てる）
                while let Some(ev) = win.event(Some(0)) {
                    match ev {
                        Event::Key(k) if k.pressed() && k.key() == key::ESCAPE => stop = true,
                        Event::Close => stop = true,
                        _ => {}
                    }
                }
                draw_status(win, msg);
                !stop
            };
            fetch::fetch(&url, &mut progress)
        };
        match result {
            Ok(r) => {
                let truncated = r.truncated;
                let page_url = r.url.clone();
                let html = match r.mime.as_str() {
                    "text/html" => {
                        let cs = charset::detect(&r.body, r.charset.as_deref());
                        Some(charset::decode(&r.body, cs))
                    }
                    _ => None,
                };
                let doc = match (html, r.mime.as_str()) {
                    (Some(h), _) => doc::parse(&h),
                    (None, "text/plain") => {
                        let cs = charset::detect(&r.body, r.charset.as_deref());
                        let mut d = doc::plain(&charset::decode(&r.body, cs));
                        d.title = file_name(&page_url);
                        d
                    }
                    (None, mime) => doc::parse(&pages::error(
                        &page_url.to_string(),
                        &format!("Cannot show this kind of file ({mime})."),
                    )),
                };
                if nav == Nav::New {
                    self.push_history(&page_url);
                } else if let Some(v) = self.history.get_mut(self.current) {
                    v.url = page_url.clone();
                }
                self.set_page(win, page_url, doc, nav);
                if truncated {
                    self.status = format!("The page is too large; showing the first {} KB.", fetch::MAX_BODY / 1024);
                }
            }
            Err(msg) => {
                let msg = if stop { String::from(fetch::STOPPED) } else { msg };
                println!("browse: {url}: {msg}");
                if stop && self.page.is_some() {
                    // 止めたら今のページのまま
                    self.status = msg;
                    return;
                }
                if nav == Nav::New {
                    self.push_history(&url);
                }
                let doc = doc::parse(&pages::error(&url.to_string(), &msg));
                self.set_page(win, url, doc, nav);
                self.status = msg;
            }
        }
    }

    /// 始めのページ（アドレス欄は空にする）
    fn show_start(&mut self, win: &mut Window, nav: Nav) {
        let url = Url { scheme: Scheme::File, host: String::new(), port: 0, path: String::from("/"), fragment: None };
        if nav == Nav::New {
            self.push_history(&url);
            if let Some(v) = self.history.last_mut() {
                v.start = true;
            }
        }
        self.set_page(win, url, doc::parse(&pages::start()), nav);
        self.ui.set_text(self.address, "");
    }

    fn set_page(&mut self, win: &mut Window, url: Url, doc: Document, nav: Nav) {
        let content = self.content();
        let layout = layout::layout(&doc, content.w, &self.fonts, &|_| None);
        let base = doc.base.as_deref().and_then(|b| url.join(b).ok()).unwrap_or_else(|| url.clone());
        let title = if doc.title.is_empty() { file_name(&url) } else { doc.title.clone() };
        win.set_title(&title);
        println!("browse: loaded {url} ({} lines) \"{title}\"", layout.lines.len());
        self.sb.set_size(layout.height, content.h);
        self.page = Some(Page { url: url.clone(), base, doc, layout });
        let y = self.target_scroll(&url, nav);
        self.sb.pos = 0;
        self.sb.set_pos(y);
        self.ui.set_text(self.address, &url.to_string());
        self.status = String::from("Done.");
    }

    /// 開いたときの位置: 履歴の中の移動なら前の位置、`#…` があればその印、ほかは先頭
    fn target_scroll(&self, url: &Url, nav: Nav) -> i32 {
        if nav == Nav::History
            && let Some(v) = self.history.get(self.current)
            && v.scroll > 0
        {
            return v.scroll;
        }
        if nav == Nav::Reload {
            return self.sb.pos;
        }
        let Some(frag) = url.fragment.as_deref() else { return 0 };
        let name = vibe_web::url::percent_decode(frag);
        self.page.as_ref().and_then(|p| p.layout.anchor(&name)).map_or(0, |y| (y - 4).max(0))
    }
}

impl Page {
    /// 下の行に出すリンクの行き先
    fn link_text(&self, l: u32) -> String {
        let href = &self.doc.links[l as usize];
        match self.base.join(href) {
            Ok(u) => u.to_string(),
            Err(e) => format!("{} ({})", href.trim(), e.message()),
        }
    }
}

/// 下の行だけを描き直す（読み込みの途中）
fn draw_status(win: &mut Window, msg: &str) {
    let (w, h) = (win.width() as i32, win.height() as i32);
    let mut c = win.canvas();
    let sy = h - STATUS_H;
    c.fill_rect(0, sy + 1, w, STATUS_H - 1, theme::SUBTLE);
    vibe_rt::gfx::draw_text(&mut c, vibe_rt::abi::FONT_UI, 6, sy + 2, msg, theme::TEXT_DIM);
    win.present();
}

/// URL のパスの最後の名前（タイトルの代わり）
fn file_name(url: &Url) -> String {
    let path = url.path.split('?').next().unwrap_or("");
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if name.is_empty() {
        match url.scheme {
            Scheme::Http => url.host.clone(),
            Scheme::File => String::from("/"),
        }
    } else {
        vibe_web::url::percent_decode(name)
    }
}
