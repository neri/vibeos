//! ドック: 画面の下の真ん中に浮かぶ、アプリのアイコンの並び（デスクトップだけ。レトロでは出さない）
//!
//! 項目は `/SYSTEM/DOCK.CFG`（1 行に 1 つのアプリの名前。`terminal` は端末）に書いた順。起動時に読み、
//! `/BIN` に無いアプリは出さない。ファイルが無いか中身が空ならドックを出さない。読み方・配置・当たり判定・
//! 作業領域は `vibe_gfx::dock`（ホストでテストする）。
//!
//! 角の丸い暗いパネルに、角の丸いタイルと 32x32 のアイコン（`ICONS32.VIA`。無ければ 16x16）を並べる。
//! そのアプリのウィンドウ（最小化したものも）があればタイルの下に点。乗せるとタイルを明るくし、上に名前の
//! 小さなパネル（ツールチップ）を出す。押すと、そのアプリのウィンドウがあれば一番手前のものを前に出し
//! （最小化していれば戻す）、無ければ起動する（メニューと同じ。ロックの外で `super::launch`）。
//! ウィンドウより手前・メニューとカーソルより奥に重ねる。パネルとツールチップの中身はそれぞれ 1 枚の
//! バッファに描いておき、重ね合わせでは `panel_row` で 1 行ずつ重ねる。描き直すのは、乗せた項目・
//! 開いているアプリが変わったときだけ。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use vibe_gfx::canvas::Canvas;
use vibe_gfx::dock::{GAP, Layout, PAD, TILE, parse_config};
use vibe_gfx::theme;

use super::deco::{self, SHADOW_MARGIN};
use super::{Face, MENU_TERMINAL, MouseAction, Screen, draw_label, label_width};

const PATH: &str = "/SYSTEM/DOCK.CFG";
/// ツールチップの高さ・文字の左右の余白・ドックとの間
const TIP_H: i32 = 24;
const TIP_PAD: i32 = 8;
const TIP_GAP: i32 = 8;
/// 開いているアプリの点の直径
const DOT: i32 = 4;

pub(super) struct DockItem {
    name: String,
    /// アイコンと 1 辺の大きさ（32x32 が無ければ 16x16）
    icon: Option<(&'static [u32], usize)>,
}

pub(super) struct Dock {
    items: Vec<DockItem>,
    layout: Layout,
    hover: Option<usize>,
    /// 項目ごとに、そのアプリのウィンドウがあるか
    running: Vec<bool>,
    /// パネルの中身（layout.w × layout.h）
    buf: Vec<u32>,
    tip: Option<Tip>,
}

/// 乗せた項目の名前のパネル
struct Tip {
    x: i32,
    y: i32,
    w: i32,
    buf: Vec<u32>,
}

/// `DOCK.CFG` と `/BIN` を読んで項目を決める（ファイルを読むので SCREEN のロックの外で）
pub(super) fn load() -> Vec<DockItem> {
    let Ok(text) = crate::fs::read_file(PATH) else {
        klog!("dock: {PATH} is not available");
        return Vec::new();
    };
    let apps: Vec<String> = crate::fs::read_dir("/BIN")
        .map(|entries| {
            entries
                .iter()
                .filter(|e| !e.is_dir())
                .filter_map(|e| e.name.strip_suffix(".ELF").map(|n| n.to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    let items: Vec<DockItem> = parse_config(core::str::from_utf8(&text).unwrap_or(""))
        .into_iter()
        .filter(|name| name == "terminal" || apps.contains(name))
        .map(|name| {
            let icon = crate::gfx::icons::get32(&name)
                .map(|i| (i, crate::gfx::icons::SIZE32))
                .or_else(|| crate::gfx::icons::for_app(&name).map(|i| (i, crate::gfx::icons::SIZE)));
            DockItem { name, icon }
        })
        .collect();
    klog!("dock: {} items", items.len());
    items
}

impl Dock {
    /// 画面が `sw` × `sh` のときのドック。項目が無いか 1 つも入らなければ None
    pub(super) fn new(items: Vec<DockItem>, sw: usize, sh: usize) -> Option<Dock> {
        let layout = Layout::new(items.len(), sw as i32, sh as i32)?;
        let running = vec![false; items.len()];
        let buf = vec![0; (layout.w * layout.h) as usize];
        Some(Dock { items, layout, hover: None, running, buf, tip: None })
    }

    /// 重ね合わせで描き直す行（影を含む）
    fn rows(&self) -> (usize, usize) {
        let top = match &self.tip {
            Some(t) => t.y,
            None => self.layout.y,
        };
        ((top - SHADOW_MARGIN).max(0) as usize, (self.layout.y + self.layout.h + SHADOW_MARGIN) as usize)
    }
}

impl Screen {
    /// 作業領域の下端（新しいウィンドウ・端末の大きさの上限。ドックがあればその分を除く）
    pub(super) fn work_bottom(&self) -> usize {
        vibe_gfx::dock::work_bottom(self.height as i32, self.dock.is_some()) as usize
    }

    /// 開いているアプリが変わっていたら描き直す（重なりの順・フォーカスが変わるたびに呼ぶ）
    pub(super) fn refresh_dock(&mut self) {
        let Some(d) = &self.dock else { return };
        let running: Vec<bool> = d.items.iter().map(|it| self.windows_of(&it.name).next().is_some()).collect();
        if let Some(d) = &mut self.dock {
            if d.running != running {
                d.running = running;
                self.render_dock();
            }
        }
    }

    /// アプリ `name` のウィンドウ（重なりの順。最小化したものも）
    fn windows_of<'a>(&'a self, name: &'a str) -> impl Iterator<Item = super::WinId> + 'a {
        self.order.iter().copied().filter(move |id| {
            self.windows.get(id).is_some_and(|w| if name == "terminal" { w.terminal } else { w.app == name })
        })
    }

    /// ドック（とツールチップ）の中身を描き直す
    pub(super) fn render_dock(&mut self) {
        let (Some(d), Some(deco)) = (&mut self.dock, &self.deco) else { return };
        let (w, h) = (d.layout.w as usize, d.layout.h as usize);
        let mut c = Canvas::new(&mut d.buf, w, h);
        c.clear(theme::DOCK);
        for i in 0..d.layout.count {
            let (tx, ty) = (PAD + i as i32 * (TILE + GAP), PAD);
            let bg = if d.hover == Some(i) { theme::FACE } else { theme::ACCENT_LIGHT };
            c.fill_round_rect(tx, ty, TILE, TILE, &deco.tile, bg, 255);
            if let Some((icon, s)) = d.items[i].icon {
                let o = (TILE - s as i32) / 2;
                c.blit_alpha(tx + o, ty + o, icon, s);
            }
            if d.running[i] {
                c.fill_round_rect(tx + (TILE - DOT) / 2, ty + TILE + 1, DOT, DOT, &deco.dot, theme::BAR_TEXT, 255);
            }
        }
        // ツールチップ
        d.tip = d.hover.map(|i| {
            let name = &d.items[i].name;
            let label = if name == "terminal" { MENU_TERMINAL } else { name.as_str() };
            let tw = label_width(Face::Ui, label) + 2 * TIP_PAD;
            let (tx, _, _, _) = d.layout.tile(i);
            let x = (tx + TILE / 2 - tw / 2).clamp(4, (self.width as i32 - tw - 4).max(4));
            let mut buf = vec![0; (tw * TIP_H) as usize];
            let mut c = Canvas::new(&mut buf, tw as usize, TIP_H as usize);
            c.clear(theme::FACE);
            draw_label(&mut c, Face::Ui, TIP_PAD, 0, TIP_H, label, theme::TEXT);
            Tip { x, y: d.layout.y - TIP_GAP - TIP_H, w: tw, buf }
        });
        let (a, b) = d.rows();
        self.mark_rows(a, b);
    }

    /// マウスの下の項目を目立たせる（`over` が None ならどれも目立たせない）
    pub(super) fn dock_hover(&mut self, over: Option<(i32, i32)>) {
        let Some(d) = &self.dock else { return };
        let hover = over.and_then(|(x, y)| d.layout.item_at(x, y));
        if hover != d.hover {
            // ツールチップが消える・動く分も描き直す
            let (a, b) = d.rows();
            self.mark_rows(a, b);
            if let Some(d) = &mut self.dock {
                d.hover = hover;
            }
            self.render_dock();
        }
    }

    /// ドックのパネルの中か
    pub(super) fn dock_contains(&self, x: i32, y: i32) -> bool {
        self.dock.as_ref().is_some_and(|d| d.layout.contains(x, y))
    }

    /// ドックを押した: 項目のアプリのウィンドウがあれば一番手前のものを前に出し、無ければ起動する
    /// （起動はロックの外で行うので、その印を返す）
    pub(super) fn dock_click(&mut self, x: i32, y: i32) -> Option<MouseAction> {
        let d = self.dock.as_ref()?;
        let name = d.items[d.layout.item_at(x, y)?].name.clone();
        let wins: Vec<super::WinId> = self.windows_of(&name).collect();
        match wins.last() {
            Some(&id) => {
                klog!("dock: raise {name} ({} windows)", wins.len());
                self.raise(id);
                None
            }
            None => {
                klog!("dock: launch {name}");
                Some(MouseAction::Launch(if name == "terminal" { String::from(MENU_TERMINAL) } else { name }))
            }
        }
    }

    /// 重ね合わせ: ドックとツールチップ（角の丸いパネルと影）の行を row に重ねる
    pub(super) fn draw_dock_row(&self, y: usize, row: &mut [u32]) {
        let (Some(d), Some(deco)) = (&self.dock, &self.deco) else { return };
        let (a, b) = d.rows();
        if y < a || y >= b {
            return;
        }
        let shadows = deco.shadows(true);
        let l = &d.layout;
        deco::panel_row(row, y as i32, (l.x, l.y, l.w, l.h), &deco.dock, &shadows, copy_from(&d.buf, y, l.x, l.y, l.w));
        if let Some(t) = &d.tip {
            let fill = copy_from(&t.buf, y, t.x, t.y, t.w);
            deco::panel_row(row, y as i32, (t.x, t.y, t.w, TIP_H), &deco.corner_small, &shadows, fill);
        }
    }
}

/// `panel_row` の中身: 幅 `w` のバッファ `buf`（左上が画面の (px, py)）の、画面の `y` 行目に当たる行を写す
fn copy_from(buf: &[u32], y: usize, px: i32, py: i32, w: i32) -> impl FnOnce(i32, &mut [u32]) + '_ {
    move |x0, span| {
        let (r, s0, w) = ((y as i32 - py) as usize, (x0 - px) as usize, w as usize);
        let n = span.len().min(w - s0);
        span[..n].copy_from_slice(&buf[r * w + s0..r * w + s0 + n]);
    }
}
