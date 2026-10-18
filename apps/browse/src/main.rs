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
//! - 画像（PNG・BMP）はページを出した後に 1 枚ずつ取ってきて、取れたものから並べ直す（Esc で止める）
//! - 読み込んだら `browse: loaded URL (N lines) "タイトル"`、画像は `browse: image URL (幅x高さ)` と
//!   `browse: images 取れた数/全部` を標準出力に出す（テストで待つ）

#![no_std]
#![no_main]

extern crate alloc;

mod fetch;
mod images;
mod pages;
mod view;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use vibe_rt::abi::{KeyEvent, key};
use vibe_rt::gfx::{Event, Window};
use vibe_rt::gfx_lib::palette::BLACK;
use vibe_ui::draw::FACE;
use vibe_rt::{println, sys};
use vibe_ui::{Action, Button, Id, Input, Rect, ScrollBar, TextField, Ui};
use vibe_web::charset;
use vibe_web::doc::{self, Document};
use vibe_web::layout::{self, Layout};
use vibe_web::url::{Scheme, Url, UrlError};

use crate::images::Picture;
use crate::view::Fonts;

vibe_rt::entry!(main);

const TOOLBAR_H: i32 = 36;
const STATUS_H: i32 = 20;
/// 下の行の文字の左端
const STATUS_X: i32 = 6;
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
    /// 読み込んだ画像（同じ URL の画像は 1 つ）
    pictures: Vec<Picture>,
    /// 文書の画像ごとの `pictures` の番号（まだ・取れなければ None）
    slots: Vec<Option<usize>>,
}

impl Page {
    /// 画像の大きさを入れて並べ直す
    fn relayout(&mut self, width: i32, fonts: &Fonts) {
        let (doc, pictures, slots) = (&self.doc, &self.pictures, &self.slots);
        let size = |i: u32| {
            let k = (*slots.get(i as usize)?)?;
            Some(images::display_size(&doc.images[i as usize], &pictures[k]))
        };
        self.layout = layout::layout(doc, width, fonts, &size);
    }
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
    /// 読み込みの間に届いたイベント（読み終わってから順に処理する）
    pending: Vec<Event>,
}

fn main() -> i32 {
    let arg = vibe_rt::args().nth(1).map(String::from);
    let start = match arg.as_deref().map(parse_target) {
        Some(Ok(u)) => Some(u),
        Some(Err(e)) => {
            println!("browse: {}", e.message());
            return 2;
        }
        None => None,
    };
    vibe_ui::set_font(vibe_rt::gfx::font16());
    let info = sys::sys_info();
    // 上端のバー（20）・タイトルバー（20）・枠と少しの余白を除いた大きさ（640x480 で 636x416）
    let w = (info.screen_width as i32 - 4).clamp(320, MAX_W);
    let h = (info.screen_height as i32 - 64).clamp(240, MAX_H);
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
        // 読み込みの間に届いたイベントを先に
        let ev = if app.pending.is_empty() { win.event(None) } else { Some(app.pending.remove(0)) };
        dirty = match ev {
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
            pending: Vec::new(),
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
        let mut c = win.surface();
        match &self.page {
            Some(p) => {
                let image = |i: u32| {
                    let k = (*p.slots.get(i as usize)?)?;
                    let pic = &p.pictures[k];
                    Some((pic.pixels.as_slice(), pic.w, pic.h))
                };
                view::draw_page(&mut c, &self.fonts, &p.layout, content, self.sb.pos, self.hover, &image)
            }
            None => c.fill_rect(content.x, content.y, content.w, content.h, view::PAGE_BG),
        }
        c.fill_rect(track.x, track.y, track.w, track.h, view::PAGE_BG);
        self.sb.draw(&mut c, track);
        // 上の道具の並びと下の行（本文の上に描く。はみ出した文字を隠す）
        c.fill_rect(0, 0, w, TOOLBAR_H, FACE);
        c.fill_rect(0, TOOLBAR_H - 2, w, 2, BLACK);
        let sy = h - STATUS_H;
        c.fill_rect(0, sy, w, STATUS_H, FACE);
        c.fill_rect(0, sy, w, 2, BLACK);
        let status = match (self.hover, &self.page) {
            (Some(l), Some(p)) => p.link_text(l),
            _ => self.status.clone(),
        };
        c.draw_text(vibe_rt::gfx::font16(), STATUS_X, sy + 3, &status, BLACK, None);
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
        let mut later = Vec::new();
        let result = {
            let mut progress = |msg: &str| {
                stop |= poll_events(win, &mut later);
                draw_status(win, msg);
                !stop
            };
            fetch::fetch(&url, &mut progress)
        };
        self.pending.extend(later);
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
                let mut picture = None;
                let doc = match (html, r.mime.as_str()) {
                    (Some(h), _) => doc::parse(&h),
                    (None, mime) if mime.starts_with("image/") => match images::decode(&r.body) {
                        Ok(p) => {
                            let name = pages::escape(&file_name(&page_url));
                            let src = pages::escape(&page_url.without_fragment().to_string());
                            let html = format!("<title>{name} ({}x{})</title><img src=\"{src}\" alt=\"{name}\">", p.w, p.h);
                            picture = Some(p);
                            doc::parse(&html)
                        }
                        Err(e) => doc::parse(&pages::error(&page_url.to_string(), e)),
                    },
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
                let pictures: Vec<Picture> = picture.into_iter().collect();
                self.set_page_with(win, page_url, doc, nav, pictures);
                if truncated {
                    self.status = format!("The page is too large; showing the first {} KB.", fetch::MAX_BODY / 1024);
                }
                self.load_images(win);
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
        self.set_page_with(win, url, doc, nav, Vec::new());
    }

    /// ページを出す。`pictures` があれば文書の最初の画像にする（画像だけのページ）
    fn set_page_with(&mut self, win: &mut Window, url: Url, doc: Document, nav: Nav, pictures: Vec<Picture>) {
        let content = self.content();
        let base = doc.base.as_deref().and_then(|b| url.join(b).ok()).unwrap_or_else(|| url.clone());
        let title = if doc.title.is_empty() { file_name(&url) } else { doc.title.clone() };
        win.set_title(&title);
        let mut slots = alloc::vec![None; doc.images.len()];
        if !pictures.is_empty() && !slots.is_empty() {
            slots[0] = Some(0);
        }
        let mut page = Page { url: url.clone(), base, doc, layout: Layout::default(), pictures, slots };
        page.relayout(content.w, &self.fonts);
        println!("browse: loaded {url} ({} lines) \"{title}\"", page.layout.lines.len());
        self.sb.set_size(page.layout.height, content.h);
        self.page = Some(page);
        let y = self.target_scroll(&url, nav);
        self.sb.pos = 0;
        self.sb.set_pos(y);
        self.ui.set_text(self.address, &url.to_string());
        self.status = String::from("Done.");
    }

    /// まだ読んでいない画像を 1 枚ずつ取ってきて、取れたら並べ直して描く。Esc で止める
    fn load_images(&mut self, win: &mut Window) {
        let Some(p) = &self.page else { return };
        let todo: Vec<(usize, Url)> = p
            .doc
            .images
            .iter()
            .enumerate()
            .filter(|(i, img)| p.slots[*i].is_none() && !img.src.is_empty())
            .filter_map(|(i, img)| p.base.join(&img.src).ok().map(|u| (i, u.without_fragment())))
            .collect();
        if todo.is_empty() {
            return;
        }
        // 画像に使ってよい画素の数（空いているメモリの 1/4）
        let mut budget = (sys::sys_info().free_kb as usize * 1024 / 4) / 4;
        let n = todo.len();
        let mut tried: Vec<(String, Option<usize>)> = Vec::new();
        let mut loaded = 0;
        for (k, (i, url)) in todo.into_iter().enumerate() {
            let key = url.to_string();
            let pic = match tried.iter().find(|(u, _)| *u == key) {
                Some(&(_, v)) => v,
                None => {
                    let mut stop = false;
                    let mut later = Vec::new();
                    let result = {
                        let mut progress = |msg: &str| {
                            stop |= poll_events(win, &mut later);
                            draw_status(win, &format!("Image {} of {n}: {msg}", k + 1));
                            !stop
                        };
                        fetch::fetch(&url, &mut progress)
                    };
                    self.pending.extend(later);
                    if stop {
                        println!("browse: images stopped");
                        self.status = String::from(fetch::STOPPED);
                        return;
                    }
                    let decoded = result.and_then(|r| images::decode(&r.body).map_err(String::from));
                    let v = match decoded {
                        Ok(pic) if pic.w * pic.h <= budget => {
                            println!("browse: image {key} ({}x{})", pic.w, pic.h);
                            budget -= pic.w * pic.h;
                            let Some(page) = &mut self.page else { return };
                            page.pictures.push(pic);
                            Some(page.pictures.len() - 1)
                        }
                        Ok(_) => {
                            println!("browse: image {key}: not enough memory");
                            None
                        }
                        Err(e) => {
                            println!("browse: image {key}: {e}");
                            None
                        }
                    };
                    tried.push((key, v));
                    v
                }
            };
            if let Some(v) = pic {
                loaded += 1;
                let content = self.content();
                if let Some(page) = &mut self.page {
                    page.slots[i] = Some(v);
                    page.relayout(content.w, &self.fonts);
                    let pos = self.sb.pos;
                    self.sb.set_size(page.layout.height, content.h);
                    self.sb.set_pos(pos);
                }
                self.draw(win);
            }
        }
        println!("browse: images {loaded}/{n}");
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

/// 読み込みの途中に届いたイベントを見る。Esc か [x] なら true（止める）。Esc のほかは `later` に取っておく
/// （[x] も、止めた後に閉じるために取っておく）
fn poll_events(win: &Window, later: &mut Vec<Event>) -> bool {
    let mut stop = false;
    while let Some(ev) = win.event(Some(0)) {
        match ev {
            Event::Key(k) if k.key() == key::ESCAPE => stop |= k.pressed(),
            Event::Close => {
                stop = true;
                later.push(ev);
            }
            ev => later.push(ev),
        }
    }
    stop
}

/// 下の行だけを描き直す（読み込みの途中）
fn draw_status(win: &mut Window, msg: &str) {
    let (w, h) = (win.width() as i32, win.height() as i32);
    let mut c = win.surface();
    let sy = h - STATUS_H;
    c.fill_rect(0, sy + 2, w, STATUS_H - 2, FACE);
    c.draw_text(vibe_rt::gfx::font16(), STATUS_X, sy + 3, msg, BLACK, None);
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
