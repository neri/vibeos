//! すりガラス（タイトルバーの後ろをぼかして透かす）の、ホストでテストできる部分:
//! 変わった所の一覧（どのガラスを計算し直すか）と、BOOT.CFG の `blur=` の読み方
//!
//! ガラスは奥から順に並ぶ（ウィンドウの重なりの順）。並びの何番目かを「深さ」と呼ぶ。
//! ガラスの後ろの画像は、それより奥の物（背景と、深さの小さいウィンドウ）だけで作るので、
//! 深さ d のガラスを計算し直すのは「d より奥の物」が「ガラスにぼかしが届く範囲」で変わったときだけ。

/// 矩形 (x, y, w, h)
pub type Rect = (i32, i32, i32, i32);

/// 背景（いちばん奥。すべてのガラスに効く）の深さ
pub const BACKGROUND: i32 = -1;

/// 1 回の重ね合わせまでに貯める変わった所の数。溢れたら「全部」にまとめる
const CAP: usize = 16;

/// 変わった所の一覧（矩形と、それがどの深さの物か）
pub struct Damage {
    items: [(Rect, i32); CAP],
    len: usize,
    /// 溢れた・全部を描き直す（すべてのガラスを計算し直す）
    all: bool,
}

impl Damage {
    pub const fn new() -> Damage {
        Damage { items: [((0, 0, 0, 0), 0); CAP], len: 0, all: false }
    }

    /// 深さ `depth` の物の矩形 `rect` が変わった。同じ深さのものがあれば矩形を合わせる
    pub fn add(&mut self, rect: Rect, depth: i32) {
        if self.all || rect.2 <= 0 || rect.3 <= 0 {
            return;
        }
        if let Some(item) = self.items[..self.len].iter_mut().find(|i| i.1 == depth) {
            item.0 = union(item.0, rect);
        } else if self.len < CAP {
            self.items[self.len] = (rect, depth);
            self.len += 1;
        } else {
            self.all = true;
        }
    }

    /// 全部が変わった（背景・重なりの順・フォーカスが変わったとき）
    pub fn add_all(&mut self) {
        self.all = true;
    }

    /// 深さ `depth` のガラスで、ぼかしが届く範囲が `reach` のものを計算し直すか
    pub fn affects(&self, reach: Rect, depth: i32) -> bool {
        self.all || self.items[..self.len].iter().any(|&(r, d)| d < depth && overlaps(r, reach))
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.all = false;
    }
}

impl Default for Damage {
    fn default() -> Self {
        Damage::new()
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let (x1, y1) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
    (x0, y0, x1 - x0, y1 - y0)
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// BOOT.CFG の文から `blur=` の値（"#" より後ろはコメント）
pub fn from_config(text: &str) -> Option<&str> {
    text.lines().filter_map(|l| l.split('#').next()?.split_once('=')).find(|(k, _)| k.trim() == "blur").map(|(_, v)| v.trim())
}

/// `blur=` の値を読む。`on`・`off` 以外は None
pub fn parse_setting(value: &str) -> Option<bool> {
    match value {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_damage_below_and_within_reach_counts() {
        let mut d = Damage::new();
        assert!(!d.affects((0, 0, 100, 100), 3));
        d.add((10, 10, 20, 20), 1);
        // 奥（1 < 2）で重なる
        assert!(d.affects((25, 25, 50, 50), 2));
        // 重ならない
        assert!(!d.affects((30, 30, 50, 50), 2));
        // 同じ深さ・手前の物は効かない
        assert!(!d.affects((0, 0, 100, 100), 1));
        assert!(!d.affects((0, 0, 100, 100), 0));
        d.clear();
        assert!(!d.affects((0, 0, 100, 100), 2));
    }

    #[test]
    fn background_affects_every_glass() {
        let mut d = Damage::new();
        d.add((0, 0, 10, 10), BACKGROUND);
        assert!(d.affects((5, 5, 1, 1), 0));
        assert!(!d.affects((20, 20, 1, 1), 0));
    }

    #[test]
    fn same_depth_is_merged_and_overflow_means_all() {
        let mut d = Damage::new();
        // 同じ深さはまとめるので何回足しても溢れない
        for i in 0..100 {
            d.add((i, 0, 1, 1), 0);
        }
        assert!(d.affects((50, 0, 1, 1), 1));
        assert!(!d.affects((50, 5, 1, 1), 1));
        for depth in 1..CAP as i32 {
            d.add((1000, 1000, 1, 1), depth);
        }
        assert!(!d.affects((500, 500, 1, 1), 100));
        // 17 個目の深さで溢れる
        d.add((1000, 1000, 1, 1), 99);
        assert!(d.affects((500, 500, 1, 1), 0));
        d.clear();
        assert!(!d.affects((500, 500, 1, 1), 100));
    }

    #[test]
    fn add_all_and_empty_rects() {
        let mut d = Damage::new();
        d.add((0, 0, 0, 10), 0);
        d.add((0, 0, 10, -1), 0);
        assert!(!d.affects((0, 0, 100, 100), 5));
        d.add_all();
        assert!(d.affects((-50, -50, 1, 1), 0));
    }

    #[test]
    fn reads_blur_setting() {
        assert_eq!(from_config("video=1024x768\nblur=off # slow\n"), Some("off"));
        assert_eq!(from_config(" blur = on"), Some("on"));
        assert_eq!(from_config("# blur=on\nwallpaper=/A.BMP"), None);
        assert_eq!(parse_setting("on"), Some(true));
        assert_eq!(parse_setting("off"), Some(false));
        assert_eq!(parse_setting("yes"), None);
    }
}
