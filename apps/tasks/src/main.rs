//! タスクマネージャ（ウィンドウ）
//!
//! タスク（カーネルスレッドとプロセス）の一覧を 1 秒ごとに読み直して出す（番号・名前・CPU の使用率・
//! メモリ・状態）。CPU の使用率は前に読んだときからの刻み数の差で求める。一覧でプロセスを選んで
//! 「End task」（か Delete キー）を押し、確かめると止める（カーネルスレッドは止められない。名前の後ろに *）。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_rt::abi::{self, ProcInfo, err, key};
use vibe_rt::gfx::{Event, Window};
use vibe_rt::{println, sys};
use vibe_ui::dialog::Dialog;
use vibe_ui::{Action, Align, Button, Id, Input, Label, List, Rect, Ui};

vibe_rt::entry!(main);

const W: usize = 320;
const H: usize = 300;
/// 一覧の列（文字の場所の左端からの (左端, 右端, 寄せ方)）と見出し
const COLUMNS: [(i32, i32, Align, &str); 5] = [
    (0, 28, Align::Right, "ID"),
    (32, 126, Align::Left, "NAME"),
    (130, 164, Align::Right, "CPU"),
    (168, 224, Align::Right, "MEM"),
    (228, 276, Align::Left, "STATE"),
];
/// 一覧の文字の場所の左端（一覧の左端 8 + 部品の余白 6）
const TEXT_X: i32 = 14;
const MAX_TASKS: usize = 64;
const REFRESH_MS: u32 = 1000;

struct App {
    ui: Ui,
    list: Id,
    end: Id,
    status: Id,
    procs: Vec<ProcInfo>,
    /// 前に読んだときの (番号, 刻み数) と、全体の刻み数
    last: Vec<(u32, u32)>,
    last_total: u32,
}

fn main() -> i32 {
    vibe_ui::set_font(vibe_rt::gfx::font16());
    let Some(mut win) = Window::open(W, H, "Tasks") else {
        println!("tasks: cannot open a window");
        return 1;
    };
    let mut app = App::new();
    app.refresh();
    println!("tasks: started");
    let mut next = sys::ticks() + REFRESH_MS / 10;
    loop {
        app.draw(&mut win);
        let wait = next.saturating_sub(sys::ticks()) * 10;
        let action = match win.event(Some(wait)) {
            Some(Event::Close) => break,
            Some(Event::Key(k)) => app.ui.handle(Input::Key(k), sys::ticks()),
            Some(Event::Mouse { x, y, buttons }) => app.ui.handle(Input::Mouse { x, y, buttons }, sys::ticks()),
            None => None,
        };
        match action {
            Some(Action::Clicked(id)) if id == app.end => app.end_task(),
            Some(Action::Key(k)) if k.pressed() && k.key() == key::DELETE => app.end_task(),
            _ => {}
        }
        if sys::ticks() >= next {
            app.refresh();
            next = sys::ticks() + REFRESH_MS / 10;
        }
    }
    drop(win);
    println!("tasks: closed");
    0
}

fn state_name(p: &ProcInfo) -> &'static str {
    match p.state {
        abi::PROC_RUNNING => "run",
        abi::PROC_READY => "ready",
        abi::PROC_BLOCKED => "wait",
        _ => "dead",
    }
}

impl App {
    fn new() -> App {
        let mut ui = Ui::new();
        // 見出し（一覧の列と同じ位置。ラベルは中で 6 ピクセル空けて描く）。白黒の画面では灰色の字（市松）は
        // 読みにくいので黒
        for (x0, x1, align, name) in COLUMNS {
            ui.add(Rect::new(TEXT_X + x0 - 6, 4, x1 - x0 + 12, 20), Label::new(name).align(align));
        }
        let list = ui.add(Rect::new(8, 26, W as i32 - 16, H as i32 - 62), List::new(Vec::new()));
        ui.list_mut(list).set_columns(COLUMNS.iter().map(|c| (c.0, c.2)).collect());
        let end = ui.add(Rect::new(8, H as i32 - 30, 90, 24), Button::new("End task"));
        let status = ui.add(Rect::new(104, H as i32 - 30, W as i32 - 112, 24), Label::new(""));
        ui.set_focus(list);
        App { ui, list, end, status, procs: Vec::new(), last: Vec::new(), last_total: 0 }
    }

    fn draw(&mut self, win: &mut Window) {
        let mut c = win.surface();
        c.clear(vibe_ui::draw::FACE);
        self.ui.draw(&mut c);
        win.present();
    }

    /// 一覧を読み直す（選んでいるタスクは番号で選び直す）
    fn refresh(&mut self) {
        let mut buf = vec![ProcInfo::EMPTY; MAX_TASKS];
        let n = sys::proc_list(&mut buf).max(0) as usize;
        buf.truncate(n);
        let info = sys::sys_info();
        let total = info.ticks.wrapping_sub(self.last_total).max(1);
        let lines: Vec<String> = buf
            .iter()
            .map(|p| {
                let before = self.last.iter().find(|l| l.0 == p.id).map_or(p.ticks, |l| l.1);
                let cpu = p.ticks.wrapping_sub(before) * 100 / total;
                let kind = if p.flags & abi::PROC_USER != 0 { ' ' } else { '*' };
                format!("{}\t{}{kind}\t{}%\t{}K\t{}", p.id, p.name(), cpu.min(100), p.mem_kb, state_name(p))
            })
            .collect();
        let idle = 100 - (info.idle_ticks.wrapping_sub(self.idle_before()) * 100 / total).min(100);
        let selected = self.selected().map(|p| p.id);
        self.last = buf.iter().map(|p| (p.id, p.ticks)).collect();
        self.last.push((u32::MAX, info.idle_ticks));
        self.last_total = info.ticks;
        self.procs = buf;
        self.ui.list_mut(self.list).set_items(lines);
        if let Some(i) = selected.and_then(|id| self.procs.iter().position(|p| p.id == id)) {
            self.ui.list_mut(self.list).select(i);
        }
        let text = format!("{} tasks CPU {idle}% {}K free", self.procs.len(), info.free_kb);
        self.ui.set_text(self.status, &text);
    }

    /// 前に読んだときのアイドルの刻み数（`last` の最後に番号 u32::MAX で入れてある）
    fn idle_before(&self) -> u32 {
        self.last.iter().find(|l| l.0 == u32::MAX).map_or(0, |l| l.1)
    }

    fn selected(&self) -> Option<ProcInfo> {
        let i = self.ui.list(self.list).selected()?;
        self.procs.get(i).copied()
    }

    fn end_task(&mut self) {
        self.ui.set_focus(self.list);
        let Some(p) = self.selected() else { return };
        if p.flags & abi::PROC_USER == 0 {
            self.ui.set_text(self.status, "Cannot end a kernel thread");
            return;
        }
        let name = String::from(p.name());
        let mut d = Dialog::yes_no(220, &format!("End {name} ({})?", p.id));
        if run_dialog(&mut d) != Some(0) {
            return;
        }
        let r = sys::kill(p.id as i32);
        if r < 0 {
            self.ui.set_text(self.status, &format!("Cannot end {name}: {}", err::name(-r)));
            return;
        }
        println!("tasks: killed {} ({name})", p.id);
        self.ui.set_text(self.status, &format!("Ended {name}"));
    }
}

/// ダイアログを別のウィンドウで出し、ボタンが押されるまで待つ
fn run_dialog(d: &mut Dialog) -> Option<usize> {
    let (w, h) = d.size();
    let mut win = Window::open(w, h, "Tasks")?;
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
