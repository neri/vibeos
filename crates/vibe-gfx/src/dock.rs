//! ドック（画面の下の真ん中に浮かぶ、アプリのアイコンの並び）のハードウェアに依存しない部分:
//! `/SYSTEM/DOCK.CFG` の読み方、項目の数と画面の大きさからの配置、座標から項目を引く当たり判定、
//! ドックに隠れない作業領域。描くのはカーネルの gui（`gui/dock.rs`）

use alloc::string::String;
use alloc::vec::Vec;

/// 1 つの項目のタイル（正方形）の 1 辺・タイルの間・パネルの内側の余白・画面の下端からの間
pub const TILE: i32 = 40;
pub const GAP: i32 = 8;
pub const PAD: i32 = 7;
pub const BOTTOM: i32 = 12;
/// パネルとタイルの角の半径
pub const RADIUS: usize = 20;
pub const TILE_RADIUS: usize = 12;
/// 画面の下からドックの分として空けておく高さ（新しいウィンドウ・端末の大きさの上限を決める作業領域）
pub const RESERVE: i32 = TILE + 2 * PAD + BOTTOM;
/// 画面の左右に残す余白（入りきらない項目は出さない）
const SIDE: i32 = 8;

/// `DOCK.CFG` の中身から項目の名前を並べる。1 行に 1 つのアプリの名前（小文字にする）、`#` より後ろは
/// コメント、空の行は飛ばす。同じ名前は最初の 1 つだけ。名前に空白があれば最初の語だけ
pub fn parse_config(text: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let Some(name) = line.split_whitespace().next() else { continue };
        let name = name.to_ascii_lowercase();
        if !items.contains(&name) {
            items.push(name);
        }
    }
    items
}

/// 画面の上のドックのパネルの位置と、出す項目の数
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub count: usize,
}

impl Layout {
    /// 項目が `n` 個のときの、`sw` × `sh` の画面での配置。1 つも入らなければ None
    pub fn new(n: usize, sw: i32, sh: i32) -> Option<Layout> {
        let fit = ((sw - 2 * SIDE - 2 * PAD + GAP) / (TILE + GAP)).max(0) as usize;
        let count = n.min(fit);
        if count == 0 || sh < RESERVE {
            return None;
        }
        let w = 2 * PAD + count as i32 * TILE + (count as i32 - 1) * GAP;
        let h = TILE + 2 * PAD;
        Some(Layout { x: (sw - w) / 2, y: sh - BOTTOM - h, w, h, count })
    }

    /// `i` 番目のタイルの矩形（画面の座標）
    pub fn tile(&self, i: usize) -> (i32, i32, i32, i32) {
        (self.x + PAD + i as i32 * (TILE + GAP), self.y + PAD, TILE, TILE)
    }

    /// パネルの中か（パネルの中を押してもウィンドウには届けない）
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// (x, y) にあるタイルの番号（タイルの間・パネルの余白は None）
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        let (dx, dy) = (x - self.x - PAD, y - self.y - PAD);
        if dx < 0 || dy < 0 || dy >= TILE {
            return None;
        }
        let i = (dx / (TILE + GAP)) as usize;
        (dx % (TILE + GAP) < TILE && i < self.count).then_some(i)
    }
}

/// 作業領域の下端（新しいウィンドウを置く・端末の大きさを決めるときの画面の下）。ドックがあればその分を除く
pub fn work_bottom(sh: i32, dock: bool) -> i32 {
    if dock { sh - RESERVE } else { sh }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_skips_comments_blank_lines_and_duplicates() {
        let cfg = "# dock\nterminal\n\n  Files  # the file manager\ncalc extra words\n#clock\ncalc\r\nmines\n";
        assert_eq!(parse_config(cfg), ["terminal", "files", "calc", "mines"]);
        assert!(parse_config("").is_empty());
        assert!(parse_config("# nothing\n   \n").is_empty());
        // 知らない名前もそのまま並べる（/BIN に無いものを除くのはカーネル）
        assert_eq!(parse_config("nosuchapp"), ["nosuchapp"]);
    }

    #[test]
    fn layout_is_centered_at_the_bottom() {
        let l = Layout::new(11, 1024, 768).unwrap();
        assert_eq!(l.count, 11);
        assert_eq!(l.w, 2 * PAD + 11 * TILE + 10 * GAP);
        assert_eq!(l.h, 54);
        assert_eq!(l.x, (1024 - l.w) / 2);
        assert_eq!(l.y + l.h + BOTTOM, 768);
        assert_eq!(l.y, 768 - RESERVE);
        assert_eq!(l.tile(0), (l.x + PAD, l.y + PAD, TILE, TILE));
        assert_eq!(l.tile(10).0 + TILE + PAD, l.x + l.w);
        // 640x480 でも 11 個入る
        assert_eq!(Layout::new(11, 640, 480).unwrap().count, 11);
    }

    #[test]
    fn layout_drops_items_that_do_not_fit() {
        let l = Layout::new(30, 640, 480).unwrap();
        assert!(l.count < 30);
        assert!(l.x >= SIDE && l.x + l.w <= 640 - SIDE);
        // もう 1 つは入らない
        assert!(2 * PAD + (l.count as i32 + 1) * TILE + l.count as i32 * GAP > 640 - 2 * SIDE);
        assert_eq!(Layout::new(0, 1024, 768), None);
        assert_eq!(Layout::new(3, 40, 768), None);
    }

    #[test]
    fn hit_test_finds_tiles_only() {
        let l = Layout::new(3, 1024, 768).unwrap();
        let (tx, ty, _, _) = l.tile(1);
        assert_eq!(l.item_at(tx, ty), Some(1));
        assert_eq!(l.item_at(tx + TILE - 1, ty + TILE - 1), Some(1));
        assert_eq!(l.item_at(tx + TILE, ty), None); // タイルの間
        assert_eq!(l.item_at(tx - 1, ty), None);
        assert_eq!(l.item_at(l.x + 1, l.y + 1), None); // パネルの余白
        assert!(l.contains(l.x + 1, l.y + 1));
        assert_eq!(l.item_at(l.x - 1, ty), None);
        assert!(!l.contains(l.x, l.y + l.h));
        let (lx, ly, _, _) = l.tile(2);
        assert_eq!(l.item_at(lx + 5, ly + 5), Some(2));
    }

    #[test]
    fn work_area_leaves_room_for_the_dock() {
        assert_eq!(work_bottom(768, true), 768 - 66);
        assert_eq!(work_bottom(768, false), 768);
        assert_eq!(RESERVE, 66);
    }
}
