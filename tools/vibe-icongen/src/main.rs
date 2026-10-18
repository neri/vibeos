//! vibe-icongen: 手で描いた白黒の 16x16 のアイコン（`assets/icons/*.txt`）を 1 つのファイル（`.VIA`。
//! crates/vibe-gfx の icons）にまとめる。
//!
//! 使い方: vibe-icongen OUT ART...（アイコンの名前はファイル名から拡張子を除いたもの、最大 12 文字）
//!
//! 絵は 16 行 × 16 文字のテキスト: `#` は黒、`.` は白、空白は透明（docs/plans/mono.md §3.6）。
//! 黒い地に出すときは、カーネルが白と黒を入れ替える。

use std::path::Path;

const SIZE: usize = 16;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: vibe-icongen OUT ART...");
        std::process::exit(2);
    }
    let mut icons = Vec::new();
    for path in &args[1..] {
        let name = Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
        assert!(name.len() <= vibe_gfx::icons::NAME_LEN, "{path}: name is longer than 12");
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let pixels = vibe_gfx::icons::parse_art(&text, SIZE).unwrap_or_else(|e| panic!("{path}: {e}"));
        icons.push((name, pixels));
    }
    icons.sort_by(|a, b| a.0.cmp(&b.0));
    let out = vibe_gfx::icons::encode(SIZE as u16, &icons);
    std::fs::write(&args[0], &out).unwrap_or_else(|e| panic!("{}: {e}", args[0]));
    eprintln!("{}: {} icons, {} bytes", args[0], icons.len(), out.len());
}
