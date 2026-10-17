//! ピクセル単位のスクロールバー（`Ui` の部品の外で使う。ブラウザの本文など）
//!
//! 全体の高さ `total` のうち `page` だけが見え、その先頭が `pos`。溝を押すと 1 画面ずつ動き、つまみは
//! ドラッグで動かせる。見た目は一覧（`List`）のスクロールバーと同じ（溝は描かず、細い丸いつまみだけ）。

use vibe_gfx::Surface;

use crate::Rect;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollBar {
    pub total: i32,
    pub page: i32,
    pub pos: i32,
    /// つまみをつかんだ所（つまみの上端からの距離）
    grab: Option<i32>,
}

impl ScrollBar {
    pub const fn new() -> ScrollBar {
        ScrollBar { total: 0, page: 0, pos: 0, grab: None }
    }

    pub fn max_pos(&self) -> i32 {
        (self.total - self.page).max(0)
    }

    /// 位置を変える（範囲に収める）。変わったら true
    pub fn set_pos(&mut self, pos: i32) -> bool {
        let pos = pos.clamp(0, self.max_pos());
        let changed = pos != self.pos;
        self.pos = pos;
        changed
    }

    /// 全体と見える高さを変える（位置は範囲に収める）
    pub fn set_size(&mut self, total: i32, page: i32) {
        self.total = total.max(0);
        self.page = page.max(0);
        self.pos = self.pos.clamp(0, self.max_pos());
    }

    /// 見えていない所があるか（無ければ描かない）
    pub fn needed(&self) -> bool {
        self.total > self.page
    }

    pub fn dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// 溝 `track` の中のつまみ（上端, 高さ）
    pub fn thumb(&self, track: Rect) -> (i32, i32) {
        let h = if self.total <= 0 { track.h } else { (track.h as i64 * self.page as i64 / self.total as i64) as i32 };
        let h = h.clamp(16.min(track.h), track.h);
        let range = (track.h - h).max(0);
        let max = self.max_pos().max(1);
        let y = track.y + (range as i64 * self.pos.min(max) as i64 / max as i64) as i32;
        (y, h)
    }

    /// マウスの入力。`press` は左のボタンを押した瞬間、`left` は押している間。位置が変わったら true
    pub fn mouse(&mut self, track: Rect, x: i32, y: i32, press: bool, left: bool) -> bool {
        if !self.needed() {
            self.grab = None;
            return false;
        }
        if press && track.contains(x, y) {
            let (ty, th) = self.thumb(track);
            if y < ty {
                return self.set_pos(self.pos - self.page);
            }
            if y >= ty + th {
                return self.set_pos(self.pos + self.page);
            }
            self.grab = Some(y - ty);
            return false;
        }
        if !left {
            self.grab = None;
            return false;
        }
        let Some(grab) = self.grab else { return false };
        let (_, th) = self.thumb(track);
        let range = (track.h - th).max(1);
        let at = (y - grab - track.y).clamp(0, range);
        let pos = (at as i64 * self.max_pos() as i64 + range as i64 / 2) / range as i64;
        self.set_pos(pos as i32)
    }

    /// 溝とつまみを描く（全体が見えていれば何も描かない）
    pub fn draw(&self, s: &mut Surface, track: Rect) {
        if !self.needed() {
            return;
        }
        let (ty, th) = self.thumb(track);
        crate::widget::draw_scrollbar(s, track, ty, th);
    }
}
