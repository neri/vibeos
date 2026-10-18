//! VGA の 16 色の番号（D-072）
//!
//! Windows の 16 色を CGA の並び（端末の ANSI の色番号と同じ）に置いたもの。値は VGA の DAC と同じ 6bit（0〜63）。
//! アプリとウィンドウのバッファはこの番号で描く。画面は白黒なので、画面に出すときは明るさの模様になる
//! （`mono`。黒 0 と白 15 はそのまま。docs/plans/mono.md）。DAC にはこの表を書いておく

pub const BLACK: u8 = 0;
pub const BLUE: u8 = 1;
pub const GREEN: u8 = 2;
pub const CYAN: u8 = 3;
pub const RED: u8 = 4;
pub const MAGENTA: u8 = 5;
pub const BROWN: u8 = 6;
pub const LIGHT_GRAY: u8 = 7;
pub const DARK_GRAY: u8 = 8;
pub const LIGHT_BLUE: u8 = 9;
pub const LIGHT_GREEN: u8 = 10;
pub const LIGHT_CYAN: u8 = 11;
pub const LIGHT_RED: u8 = 12;
pub const LIGHT_MAGENTA: u8 = 13;
pub const YELLOW: u8 = 14;
pub const WHITE: u8 = 15;

/// VGA のモード 12h の 16 色（各 6bit。DECISIONS.md D-072）。Windows の 16 色を CGA の並び（端末の ANSI の
/// 色番号と同じ。0〜15 の名前の定数がそのまま使える）に置いたもの。画面の白黒の模様の段（`mono::LEVEL`）は
/// この色の明るさで決めた
pub const VGA16: [[u8; 3]; 16] = [
    [0, 0, 0],    // 黒
    [0, 0, 32],   // 紺
    [0, 32, 0],   // 緑
    [0, 32, 32],  // 青緑
    [32, 0, 0],   // えんじ
    [32, 0, 32],  // 紫
    [32, 32, 0],  // オリーブ
    [48, 48, 48], // 明るい灰（192）
    [32, 32, 32], // 濃い灰（128）
    [0, 0, 63],   // 青
    [0, 63, 0],   // 明るい緑
    [0, 63, 63],  // 水色
    [63, 0, 0],   // 赤
    [63, 0, 63],  // 明るい紫
    [63, 63, 0],  // 黄
    [63, 63, 63], // 白
];
