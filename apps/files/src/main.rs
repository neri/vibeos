//! ファイルマネージャ（ウィンドウ）
//!
//! フォルダの中身を一覧にし、ダブルクリック（か Enter）でフォルダに入る・ファイルを開く。
//! 開き方は拡張子で決まる（`vibe_files::ASSOCIATIONS`: BMP → view、HTM → browse、テキスト → 新しい端末で edit、
//! ELF → 実行）。下のボタンで開く・コピー・名前の変更（移動）・削除・フォルダを作る。
//! 変更したらフロッピーにも書き戻す（sync）。BackSpace で親のフォルダへ、Delete で削除。
//! 起動時の引数でフォルダを指定できる（無ければ "/"）。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_files::{Entry, Opener};
use vibe_rt::abi::{self, DirEntry, KeyEvent, err, key};
use vibe_rt::gfx::{Event, Window};
use vibe_rt::{println, sys};
use vibe_ui::dialog::Dialog;
use vibe_ui::{Action, Button, Id, Input, Label, List, Rect, Ui};

vibe_rt::entry!(main);

const W: usize = 320;
const H: usize = 224;
const DIALOG_W: usize = 240;
const MAX_ENTRIES: usize = 256;

struct App {
    ui: Ui,
    path_label: Id,
    list: Id,
    status: Id,
    open: Id,
    copy: Id,
    rename: Id,
    delete: Id,
    folder: Id,
    dir: String,
    entries: Vec<Entry>,
    /// 一覧のアイコン（名前 → 16 色の 16x16）。起動時にカーネルから 1 回取る
    icons: Vec<(&'static str, Option<&'static [u8]>)>,
}

/// 一覧に出すアイコン（vibe_files::icon_name が返す名前）
const ICON_NAMES: [&str; 6] = ["folder", "file", "image", "script", "app", "browse"];

fn main() -> i32 {
    let start = vibe_rt::args().nth(1).map(|p| vibe_files::resolve("/", p)).unwrap_or_else(|| String::from("/"));
    vibe_ui::set_font(vibe_rt::gfx::font16());
    let Some(mut win) = Window::open(W, H, "Files") else {
        println!("files: cannot open a window");
        return 1;
    };
    let mut app = App::new();
    if !app.chdir(&start) {
        app.chdir("/");
    }
    println!("files: started");
    loop {
        app.draw(&mut win);
        let action = match win.event(None) {
            Some(Event::Close) | None => break,
            Some(Event::Key(k)) => app.ui.handle(Input::Key(k), sys::ticks()),
            Some(Event::Mouse { x, y, buttons }) => app.ui.handle(Input::Mouse { x, y, buttons }, sys::ticks()),
        };
        if let Some(a) = action {
            app.act(a);
        }
    }
    drop(win);
    println!("files: closed");
    0
}

impl App {
    fn new() -> App {
        let mut ui = Ui::new();
        let path_label = ui.add(Rect::new(8, 4, W as i32 - 16, 20), Label::new("/"));
        let list = ui.add(Rect::new(8, 26, W as i32 - 16, 144), List::new(Vec::new()));
        // 名前と大きさ（右寄せ）の 2 列
        ui.list_mut(list).set_columns(vec![(0, vibe_ui::Align::Left), (180, vibe_ui::Align::Right)]);
        let cells = Rect::new(8, 176, W as i32 - 16, 24).grid(5, 1, 6);
        let open = ui.add(cells[0], Button::new("Open"));
        let copy = ui.add(cells[1], Button::new("Copy"));
        let rename = ui.add(cells[2], Button::new("Rename"));
        let delete = ui.add(cells[3], Button::new("Delete"));
        let folder = ui.add(cells[4], Button::new("Folder"));
        let status = ui.add(Rect::new(8, 202, W as i32 - 16, 20), Label::new(""));
        ui.set_focus(list);
        let icons = ICON_NAMES
            .iter()
            .map(|&n| (n, vibe_rt::gfx::icon16(n).map(|px| &*alloc::boxed::Box::leak(px.into_boxed_slice()))))
            .collect();
        App {
            ui,
            path_label,
            list,
            status,
            open,
            copy,
            rename,
            delete,
            folder,
            dir: String::from("/"),
            entries: Vec::new(),
            icons,
        }
    }

    fn draw(&mut self, win: &mut Window) {
        let mut s = win.surface();
        s.clear(vibe_ui::draw::FACE);
        self.ui.draw(&mut s);
        win.present();
    }

    fn message(&mut self, text: &str) {
        self.ui.set_text(self.status, text);
    }

    /// フォルダ `path` に移る。読めなければ false
    fn chdir(&mut self, path: &str) -> bool {
        let Some(entries) = read_entries(path) else {
            self.message(&format!("Cannot open {path}"));
            return false;
        };
        self.dir = String::from(path);
        self.entries = entries;
        self.ui.set_text(self.path_label, &self.dir.clone());
        let items = vibe_files::list_items(&self.dir, &self.entries);
        let icon = |name: &str| self.icons.iter().find(|i| i.0 == name).and_then(|i| i.1);
        let mut icons: Vec<Option<&'static [u8]>> = Vec::new();
        if items.len() > self.entries.len() {
            icons.push(icon("folder")); // ".."
        }
        icons.extend(self.entries.iter().map(|e| icon(vibe_files::icon_name(e))));
        self.ui.list_mut(self.list).set_items(items);
        self.ui.list_mut(self.list).set_icons(icons);
        println!("files: dir {}", self.dir);
        true
    }

    /// 今のフォルダを読み直し、`select` の名前の項目を選ぶ
    fn refresh(&mut self, select: Option<&str>) {
        let dir = self.dir.clone();
        let old = self.ui.list(self.list).selected();
        if !self.chdir(&dir) {
            return;
        }
        let index = match select {
            Some(name) => vibe_files::index_of(&self.dir, &self.entries, name),
            None => old,
        };
        if let Some(i) = index {
            // 消した後は、同じ位置（末尾を超えれば末尾）を選ぶ
            let last = self.ui.list(self.list).items().len().saturating_sub(1);
            self.ui.list_mut(self.list).select(i.min(last));
        }
    }

    /// 選んでいる項目（".." なら None）
    fn selected(&self) -> Option<Entry> {
        let i = self.ui.list(self.list).selected()?;
        vibe_files::entry_at(&self.dir, &self.entries, i).cloned()
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Activated(id, i) if id == self.list => self.activate(i),
            Action::Clicked(id) if id == self.open => {
                if let Some(i) = self.ui.list(self.list).selected() {
                    self.activate(i);
                }
            }
            Action::Clicked(id) if id == self.copy => self.copy(),
            Action::Clicked(id) if id == self.rename => self.rename(),
            Action::Clicked(id) if id == self.delete => self.delete(),
            Action::Clicked(id) if id == self.folder => self.make_folder(),
            Action::Key(k) if k.pressed() => self.shortcut(k),
            _ => {}
        }
        // ボタンを押した後もキーボードで一覧を操作できるように
        if matches!(action, Action::Clicked(_)) {
            self.ui.set_focus(self.list);
        }
    }

    fn shortcut(&mut self, k: KeyEvent) {
        match k.key() {
            key::BACKSPACE => self.go_up(),
            key::DELETE => self.delete(),
            _ => {}
        }
    }

    fn go_up(&mut self) {
        if self.dir == "/" {
            return;
        }
        let from = String::from(vibe_files::file_name(&self.dir));
        let up = vibe_files::parent(&self.dir);
        if self.chdir(&up) {
            if let Some(i) = vibe_files::index_of(&self.dir, &self.entries, &from) {
                self.ui.list_mut(self.list).select(i);
            }
        }
    }

    /// 一覧の `i` 番目を開く（".." なら親へ、フォルダなら入る、ファイルなら拡張子で決まるアプリで）
    fn activate(&mut self, i: usize) {
        let Some(e) = vibe_files::entry_at(&self.dir, &self.entries, i).cloned() else {
            self.go_up();
            return;
        };
        let path = vibe_files::join(&self.dir, &e.name);
        if e.is_dir {
            self.chdir(&path);
            return;
        }
        let result = match vibe_files::opener_for(&path) {
            Some(Opener::App(app)) => start(app, &path).map(|_| String::from(app)),
            Some(Opener::Terminal(app)) => {
                let r = sys::term_open(&format!("{app} {path}"));
                if r < 0 { Err(-r) } else { Ok(format!("{app} in a terminal")) }
            }
            Some(Opener::Run) => start(&path, "").map(|_| String::from("itself")),
            None => {
                self.message(&format!("No app for {}", e.name));
                return;
            }
        };
        match result {
            Ok(how) => {
                println!("files: opened {path} with {how}");
                self.message(&format!("Opened {}", e.name));
            }
            Err(code) => self.message(&format!("Cannot open {}: {}", e.name, err::name(code))),
        }
    }

    fn copy(&mut self) {
        let Some(e) = self.selected() else { return };
        if e.is_dir {
            self.message("Cannot copy a folder");
            return;
        }
        let Some(to) = self.ask_path(&format!("Copy {} to:", e.name), "") else { return };
        let from = vibe_files::join(&self.dir, &e.name);
        match copy_file(&from, &to) {
            Ok(()) => {
                let note = sync_note();
                println!("files: copied {from} to {to}");
                self.refresh(Some(vibe_files::file_name(&to)));
                self.message(&format!("Copied to {to}{note}"));
            }
            Err(code) => self.message(&format!("Cannot copy: {}", err::name(code))),
        }
    }

    fn rename(&mut self) {
        let Some(e) = self.selected() else { return };
        let Some(to) = self.ask_path(&format!("Rename {} to:", e.name), &e.name) else { return };
        let from = vibe_files::join(&self.dir, &e.name);
        let r = sys::rename(&from, &to);
        if r < 0 {
            self.message(&format!("Cannot rename: {}", err::name(-r)));
            return;
        }
        let note = sync_note();
        println!("files: renamed {from} to {to}");
        self.refresh(Some(vibe_files::file_name(&to)));
        self.message(&format!("Renamed to {to}{note}"));
    }

    fn delete(&mut self) {
        let Some(e) = self.selected() else { return };
        let kind = if e.is_dir { "folder" } else { "file" };
        if self.ask(&format!("Delete the {kind} {}?", e.name), &["Yes", "No"]) != Some(0) {
            return;
        }
        let path = vibe_files::join(&self.dir, &e.name);
        let r = sys::remove(&path);
        if r < 0 {
            self.message(&format!("Cannot delete: {}", err::name(-r)));
            return;
        }
        let note = sync_note();
        println!("files: deleted {path}");
        self.refresh(None);
        self.message(&format!("Deleted {}{note}", e.name));
    }

    fn make_folder(&mut self) {
        let Some(path) = self.ask_path("New folder name:", "") else { return };
        let r = sys::mkdir(&path);
        if r < 0 {
            self.message(&format!("Cannot make the folder: {}", err::name(-r)));
            return;
        }
        let note = sync_note();
        println!("files: made folder {path}");
        self.refresh(Some(vibe_files::file_name(&path)));
        self.message(&format!("Made {path}{note}"));
    }

    /// 名前（今のフォルダからの相対か絶対パス）を聞いて、絶対パスにして返す
    fn ask_path(&mut self, text: &str, initial: &str) -> Option<String> {
        let (_, input) = self.ask_with_text(text, initial)?;
        let input = input.trim();
        (!input.is_empty()).then(|| vibe_files::resolve(&self.dir, input))
    }

    fn ask_with_text(&mut self, text: &str, initial: &str) -> Option<(usize, String)> {
        let mut d = Dialog::prompt(DIALOG_W, text, initial, &["OK", "Cancel"]);
        match run_dialog(&mut d)? {
            0 => Some((0, String::from(d.input()))),
            _ => None,
        }
    }

    /// はい/いいえなどを聞く。押したボタンの番号（閉じたら None）
    fn ask(&mut self, text: &str, buttons: &[&str]) -> Option<usize> {
        let mut d = Dialog::new(DIALOG_W, text, buttons);
        run_dialog(&mut d)
    }
}

/// システムのクリップボード（ダイアログのテキスト欄で使う）
struct SysClipboard;

impl vibe_ui::Clipboard for SysClipboard {
    fn get(&mut self) -> String {
        sys::clip_get()
    }
    fn set(&mut self, text: &str) {
        sys::clip_set(text);
    }
}

/// ダイアログを別のウィンドウで出し、ボタンが押されるまで待つ
fn run_dialog(d: &mut Dialog) -> Option<usize> {
    d.ui.set_clipboard(alloc::boxed::Box::new(SysClipboard));
    let (w, h) = d.size();
    let mut win = Window::open(w, h, "Files")?;
    loop {
        d.draw(&mut win.surface());
        win.present();
        let input = match win.event(None)? {
            Event::Close => return None,
            Event::Key(k) => Input::Key(k),
            Event::Mouse { x, y, buttons } => Input::Mouse { x, y, buttons },
        };
        if let Some(i) = d.handle(input, sys::ticks()) {
            return Some(i);
        }
    }
}

/// アプリを起動して、終わるのを待たない
fn start(app: &str, args: &str) -> Result<(), i32> {
    let pid = sys::spawn(app, args);
    if pid < 0 {
        return Err(-pid);
    }
    sys::detach(pid);
    Ok(())
}

/// フロッピーに書き戻す（終わってから知らせる。テストはその知らせでディスクを見る）。
/// ドライブが無ければ、そのことを書き添える文字列
fn sync_note() -> &'static str {
    if sys::sync() < 0 { " (memory only)" } else { "" }
}

fn read_entries(path: &str) -> Option<Vec<Entry>> {
    let mut raw = vec![DirEntry::EMPTY; MAX_ENTRIES];
    let n = sys::readdir(path, &mut raw);
    if n < 0 {
        return None;
    }
    let mut entries: Vec<Entry> = raw[..n as usize]
        .iter()
        .map(|d| Entry { name: String::from(d.name()), is_dir: d.is_dir != 0, size: d.size })
        .collect();
    vibe_files::sort_entries(&mut entries);
    Some(entries)
}

/// ファイルをコピーする（同じ名前があれば EEXIST）
fn copy_file(from: &str, to: &str) -> Result<(), i32> {
    let mut st = abi::Stat::default();
    if sys::stat(to, &mut st) >= 0 {
        return Err(err::EEXIST);
    }
    let data = vibe_rt::read_file(from)?;
    let fd = sys::open_with(to, abi::OPEN_WRITE | abi::OPEN_CREATE | abi::OPEN_TRUNCATE);
    if fd < 0 {
        return Err(-fd);
    }
    let n = sys::write(fd, &data);
    sys::close(fd);
    if n < 0 {
        return Err(-n);
    }
    Ok(())
}
