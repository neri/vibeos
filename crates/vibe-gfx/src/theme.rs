//! デスクトップの見た目（色・角の丸み・影）。カーネルの gui とアプリの部品（vibe-ui）で同じものを使う
//!
//! 明るい地に強調色 1 色（紫）。文字は黒ではなく濃い灰色（DECISIONS.md D-047）。
//! ウィンドウ・メニュー・部品は縁を描かず、ウィンドウは 2 層の影で背景と、部品は薄い灰色の塗りで地と分ける。
//! 灰色は少し温かい色（D-057、Aster のモックアップ）

use crate::backdrop::Circle;
use crate::canvas::{Color, rgb};

/// 強調色（フォーカスの輪・既定のボタン・チェック）と、その薄い色（選んだもの・フォーカスのあるバーのボタンの地）
pub const ACCENT: Color = rgb(83, 74, 183);
pub const ACCENT_LIGHT: Color = rgb(206, 203, 246);
/// 薄い強調色の地の上の文字（濃い強調色）
pub const ACCENT_TEXT: Color = rgb(38, 33, 92);
/// 既定のボタンにマウスを乗せた・押したときの地
pub const ACCENT_HOVER: Color = rgb(101, 92, 201);
pub const ACCENT_PRESSED: Color = rgb(66, 58, 152);
/// 文字と、薄い文字（フォーカスの無いウィンドウの題名・最小化したウィンドウ）
pub const TEXT: Color = rgb(44, 44, 42);
pub const TEXT_DIM: Color = rgb(136, 135, 128);
/// ウィンドウ・パネルの地（タイトルバーも。フォーカスの有無で変えない）
pub const FACE: Color = rgb(255, 255, 255);
/// 薄い灰色の地（タイトルバーのボタン・部品の塗り・一覧の地）
pub const SUBTLE: Color = rgb(241, 239, 232);
/// マウスを乗せたボタンの地と、押したボタンの地（もう一段濃い）、閉じるボタンの地
pub const HOVER: Color = rgb(211, 209, 199);
pub const PRESSED: Color = rgb(190, 187, 175);
pub const CLOSE_HOVER: Color = rgb(226, 67, 72);
/// 端末の暗いタイトルバーの文字（フォーカスの有無）とボタンの地（ふつう・乗せたとき）
pub const DARK_TEXT: Color = rgb(222, 221, 214);
pub const DARK_TEXT_DIM: Color = rgb(128, 128, 122);
pub const DARK_BUTTON: Color = rgb(68, 68, 65);
pub const DARK_HOVER: Color = rgb(95, 94, 90);
/// 上端のバーの文字（地は塗らず背景の上に描く）と、最小化したウィンドウの文字
pub const BAR_TEXT: Color = rgb(238, 237, 254);
pub const BAR_TEXT_DIM: Color = rgb(168, 162, 216);
/// 上端のバーのフォーカスのあるウィンドウ・開いているメニューのボタンの地（白をこの濃さで重ねる）
pub const BAR_PILL_ALPHA: u8 = 51;
/// 背景の画像の上のバーに重ねる黒の濃さ（明るい画像でも白い文字が読めるように。35%）
pub const BAR_SHADE_ALPHA: u8 = 89;
/// ドックのパネルの地（濃い紫）
pub const DOCK: Color = rgb(38, 33, 92);
/// 既定の背景: 紫の地に、同じ系統の色の大きな円（右上に明るい紫、左下に濃い紫）
pub const DESKTOP: Color = rgb(60, 52, 137);
pub const DESKTOP_CIRCLES: [Circle; 2] = [
    Circle { cx: 880, cy: 100, r: 660, color: rgb(83, 74, 183) },
    Circle { cx: 100, cy: 960, r: 375, color: rgb(38, 33, 92) },
];
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
pub const RADIUS: usize = 14;
pub const RADIUS_SMALL: usize = 6;
/// 影は 2 層: 近い影（小さくぼかして少し下へ）と遠い影（大きくぼかして下へ）。色は黒でなく背景の暗い色
pub const SHADOW_COLOR: Color = rgb(4, 0, 30);
pub const SHADOW_NEAR_BLUR: usize = 3;
pub const SHADOW_NEAR_DY: i32 = 1;
pub const SHADOW_FAR_BLUR: usize = 20;
pub const SHADOW_FAR_DY: i32 = 10;
/// フォーカスの無いウィンドウの遠い影は下へのずれが小さい（低く浮いて見える）
pub const SHADOW_FAR_DY_INACTIVE: i32 = 6;
/// 影の濃さ（近い影, 遠い影）。フォーカスのあるウィンドウ・メニューと、フォーカスの無いウィンドウ
pub const SHADOW_ALPHA: (u8, u8) = (64, 128);
pub const SHADOW_ALPHA_INACTIVE: (u8, u8) = (50, 72);
