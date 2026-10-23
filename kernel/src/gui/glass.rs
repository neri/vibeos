//! すりガラスのタイトルバー（BOOT.CFG の `blur=on`。docs/plans/title-blur.md）
//!
//! タイトルバーの後ろにある物（背景と、そのウィンドウより奥のウィンドウ）を、タイトルバーの矩形を上下左右に
//! ぼかしの届く幅だけ広げた範囲で 1 行ずつ合成し、ガウスぼかし（`vibe_gfx::blur`）をかけて、白（端末は暗い色）を
//! 重ねたものを `Window::glass` に取っておく。タイトルバーはそれを写してから題名・ボタンを描く（`render_titlebar`）。
//!
//! 1 行ずつの重ね合わせの中では上下の行が無いので、`present` で行を合成する前に、計算し直す必要のあるものを
//! 奥から順に計算する。奥のタイトルバーもガラスなので、奥から順にすれば手前は計算し直した奥を透かして見る。
//! 計算し直すのは、変わった所の一覧（`vibe_gfx::glass::Damage`）に「奥の物がぼかしの届く範囲で変わった」もの
//! があるとき・ウィンドウが動いたとき・まだ作っていないときだけ。

use alloc::vec::Vec;

use vibe_gfx::blur::Blur;
use vibe_gfx::canvas::blend;
use vibe_gfx::glass::{self, Rect};
use vibe_gfx::theme;

use super::{BAR_H, Mode, Screen, TITLE_H, WinId, render_titlebar};

/// ぼかしの作業用のバッファ（デスクトップにするときに作る。blur=off なら作らない）
pub(super) struct Glass {
    blur: Blur,
    /// 後ろの画像（ぼかしの範囲の幅 × 高さ）
    src: Vec<u32>,
    /// ぼかしの作業用（縦・横の 1 本分 × 2）
    tmp: Vec<u32>,
    /// 画面の 1 行（合成用）
    row: Vec<u32>,
}

impl Glass {
    pub(super) fn new(width: usize) -> Glass {
        Glass { blur: Blur::new(theme::GLASS_BLUR_K), src: Vec::new(), tmp: Vec::new(), row: alloc::vec![0; width] }
    }

    /// ぼかしが届く幅（片側）
    fn margin(&self) -> i32 {
        self.blur.reach() as i32
    }
}

/// BOOT.CFG の `blur=` を読む（ファイルを読むので SCREEN のロックの外で）。既定は off
pub(super) fn enabled_in_config() -> bool {
    let text = crate::fs::read_file("/BOOT.CFG").unwrap_or_default();
    let Some(value) = glass::from_config(core::str::from_utf8(&text).unwrap_or("")) else { return false };
    glass::parse_setting(value).unwrap_or_else(|| {
        klog!("gui: cannot read blur={value}, using on");
        true
    })
}

impl Screen {
    /// ウィンドウ `id` のタイトルバーの矩形
    fn titlebar_rect(&self, id: WinId) -> Option<Rect> {
        let w = self.windows.get(&id)?;
        Some((w.x, w.y - TITLE_H as i32, w.w as i32, TITLE_H as i32))
    }

    /// 重なりの順での位置（ガラスの深さ）
    pub(super) fn depth_of(&self, id: WinId) -> i32 {
        self.order.iter().position(|&w| w == id).map_or(glass::BACKGROUND, |i| i as i32)
    }

    /// ウィンドウ `id` の矩形 `rect` が変わった（ガラスを計算し直すかを決めるのに使う）
    pub(super) fn damage_window(&mut self, id: WinId, rect: Rect) {
        let depth = self.depth_of(id);
        self.damage.add(rect, depth);
    }

    /// ウィンドウ `id` のタイトルバーを描き直した
    pub(super) fn damage_titlebar(&mut self, id: WinId) {
        if let Some(r) = self.titlebar_rect(id) {
            self.damage_window(id, r);
        }
    }

    /// 計算し直す必要のあるガラスを奥から順に計算し、タイトルバーを描き直す（`present` の最初に呼ぶ）
    pub(super) fn update_glass(&mut self) {
        if self.mode != Mode::Desktop {
            self.damage.clear();
            return;
        }
        let Some(mut g) = self.glass.take() else {
            self.damage.clear();
            return;
        };
        let focused = self.focused();
        let m = g.margin();
        for i in 0..self.order.len() {
            let id = self.order[i];
            let w = &self.windows[&id];
            if w.minimized {
                continue;
            }
            let tb = (w.x, w.y - TITLE_H as i32, w.w as i32, TITLE_H as i32);
            let reach = (tb.0 - m, tb.1 - m, tb.2 + 2 * m, tb.3 + 2 * m);
            let stale = w.glass_stale || w.glass.len() != w.w * TITLE_H;
            if !stale && !self.damage.affects(reach, i as i32) {
                continue;
            }
            let dark = w.terminal;
            let Some(win) = self.windows.get_mut(&id) else { continue };
            let mut out = core::mem::take(&mut win.glass);
            self.compute(&mut g, i, tb, dark, &mut out);
            let hover = self.hover.filter(|h| h.0 == id).map(|h| h.1);
            let (Some(win), Some(deco)) = (self.windows.get_mut(&id), self.deco.as_deref()) else { continue };
            win.glass = out;
            win.glass_stale = false;
            render_titlebar(win, focused == Some(id), hover, deco);
            // 手前のガラスはこのタイトルバーを透かして見る
            self.damage.add(tb, i as i32);
            self.mark_rows(tb.1.max(0) as usize, (tb.1 + tb.3).max(0) as usize);
        }
        self.glass = Some(g);
        self.damage.clear();
    }

    /// 重なりの順で `depth` 番目のウィンドウのタイトルバー `tb` のガラスを `out` に作る
    fn compute(&self, g: &mut Glass, depth: usize, tb: Rect, dark: bool, out: &mut Vec<u32>) {
        let (sw, sh) = (self.width as i32, self.height as i32);
        let m = g.margin();
        let (x0, x1) = ((tb.0 - m).max(0), (tb.0 + tb.2 + m).min(sw));
        let (y0, y1) = ((tb.1 - m).max(0), (tb.1 + tb.3 + m).min(sh));
        let (tint, alpha) = if dark {
            (theme::TERMINAL[0], theme::GLASS_DARK_TINT_ALPHA)
        } else {
            (theme::FACE, theme::GLASS_TINT_ALPHA)
        };
        out.clear();
        if x0 >= x1 || y0 >= y1 {
            // 画面の外（ここには来ないはず）: 重ねる色だけ
            out.resize((tb.2 * tb.3) as usize, tint);
            return;
        }
        let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        g.src.resize(rw * rh, 0);
        g.tmp.resize(2 * rw.max(rh), 0);
        // 後ろの画像: 背景と、このウィンドウより奥のウィンドウ（上端のバーの行は背景だけ）
        let focused = self.focused();
        for (r, y) in (y0..y1).enumerate() {
            let row = &mut g.row[..self.width];
            self.background_row(y as usize, row);
            if y >= BAR_H as i32 {
                for &id in &self.order[..depth] {
                    self.draw_window_row(id, y, row, focused == Some(id));
                }
            }
            g.src[r * rw..(r + 1) * rw].copy_from_slice(&row[x0 as usize..x1 as usize]);
        }
        // ぼかす（タイトルバーの所だけ正しく計算する）
        let tx = ((tb.0.max(x0) - x0) as usize, ((tb.0 + tb.2).min(x1) - x0).max(0) as usize);
        let ty = ((tb.1.max(y0) - y0) as usize, ((tb.1 + tb.3).min(y1) - y0).max(0) as usize);
        if tx.0 < tx.1 && ty.0 < ty.1 {
            g.blur.apply_region(&mut g.src, rw, rh, &mut g.tmp, (tx.0, ty.0, tx.1, ty.1));
        }
        // タイトルバーの大きさに切り出して色を重ねる（画面の外の列は端の色）
        out.reserve((tb.2 * tb.3) as usize);
        for y in tb.1..tb.1 + tb.3 {
            let sy = (y.clamp(y0, y1 - 1) - y0) as usize;
            let line = &g.src[sy * rw..(sy + 1) * rw];
            out.extend((tb.0..tb.0 + tb.2).map(|x| blend(line[(x.clamp(x0, x1 - 1) - x0) as usize], tint, alpha)));
        }
    }
}
