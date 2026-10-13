//! デスクトップの見た目（色・角の丸み・影）。カーネルの gui とアプリの部品（vibe-ui）で同じものを使う
//!
//! 明るい地に強調色 1 色（青）。文字は黒ではなく濃い灰色（DECISIONS.md D-047）

use crate::canvas::{Color, rgb};

/// 強調色（フォーカス・選んだもの）と、その薄い色（地に敷く）
pub const ACCENT: Color = rgb(37, 99, 235);
pub const ACCENT_LIGHT: Color = rgb(219, 230, 252);
/// 文字と、薄い文字（フォーカスの無いウィンドウの題名・最小化したウィンドウ）
pub const TEXT: Color = rgb(32, 33, 36);
pub const TEXT_DIM: Color = rgb(128, 131, 138);
/// ウィンドウ・パネルの地（フォーカスのあるタイトルバーも）と、フォーカスの無いタイトルバー
pub const FACE: Color = rgb(250, 250, 251);
pub const FACE_INACTIVE: Color = rgb(232, 233, 237);
/// ウィンドウの 1 ピクセルの縁と、区切りの線
pub const BORDER: Color = rgb(176, 179, 186);
pub const SEPARATOR: Color = rgb(218, 220, 225);
/// マウスを乗せたボタンの地と、閉じるボタンの地
pub const HOVER: Color = rgb(222, 224, 230);
pub const CLOSE_HOVER: Color = rgb(226, 67, 72);
/// 上端のバー
pub const BAR: Color = rgb(244, 245, 248);
/// 背景のグラデーション（上 → 下）
pub const DESKTOP_TOP: Color = rgb(64, 112, 196);
pub const DESKTOP_BOTTOM: Color = rgb(146, 104, 184);
/// デスクトップの端末の 16 色（CGA の並び: 黒 青 緑 水 赤 紫 茶 明るい灰 / 濃い灰 明るい青 …）。
/// 0 番（黒）は端末の地、7 番（明るい灰）は文字の色
pub const TERMINAL: [Color; 16] = [
    rgb(26, 28, 34),
    rgb(64, 126, 240),
    rgb(78, 186, 96),
    rgb(56, 178, 196),
    rgb(226, 82, 82),
    rgb(176, 104, 214),
    rgb(212, 160, 52),
    rgb(208, 211, 218),
    rgb(112, 118, 130),
    rgb(120, 168, 255),
    rgb(130, 222, 140),
    rgb(110, 218, 230),
    rgb(255, 120, 116),
    rgb(222, 150, 240),
    rgb(244, 214, 96),
    rgb(246, 247, 250),
];

/// 角の半径（ウィンドウ・パネル）と、小さな部品の角の半径
pub const RADIUS: usize = 8;
pub const RADIUS_SMALL: usize = 4;
/// 影のぼかしの幅・下へのずれ・濃さ（フォーカスのあるウィンドウとそれ以外）
pub const SHADOW_BLUR: usize = 12;
pub const SHADOW_DY: i32 = 4;
pub const SHADOW_ALPHA: u8 = 100;
pub const SHADOW_ALPHA_INACTIVE: u8 = 55;
