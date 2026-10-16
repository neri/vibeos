//! VGA のモード 12h（640x480・16 色）の 4 枚の面への並べ替え
//!
//! モード 12h の VRAM は色のビットごとに 4 枚の面に分かれていて、どの面も 1 バイトに 8 ピクセル
//! （最上位ビットが左端）を持つ。色番号（0〜15）の 1 行を、面ごとのバイトの並びにする。
//! ハードウェアには触らない（書き込みは kernel/src/drivers/vga.rs）。

/// 色番号 `c` の面 p のビットを、`u32` の p 番目のバイトの最上位ビットに置いたもの
const SPREAD: [u32; 16] = {
    let mut t = [0u32; 16];
    let mut c = 0;
    while c < 16 {
        let mut p = 0;
        while p < 4 {
            if c >> p & 1 != 0 {
                t[c] |= 0x80 << (8 * p);
            }
            p += 1;
        }
        c += 1;
    }
    t
};

/// 8 ピクセル（足りなければ残りは 0 番の色）を 4 枚の面の 1 バイトずつにする。
/// 返り値の下位のバイトから面 0〜3。色番号の上位 4 ビットは見ない
pub fn pack8(px: &[u8]) -> u32 {
    let mut v = 0;
    for (i, &c) in px.iter().take(8).enumerate() {
        // 最上位ビットを右へ i ずらしても、同じバイトの中に収まる
        v |= SPREAD[(c & 15) as usize] >> i;
    }
    v
}

/// 1 行の色番号を 8 ピクセルずつ `pack8` して `out` に並べる。書いた数（`row` の 8 ピクセル単位の切り上げと
/// `out` の長さの小さい方）を返す
pub fn pack_row(row: &[u8], out: &mut [u32]) -> usize {
    let mut n = 0;
    for (o, px) in out.iter_mut().zip(row.chunks(8)) {
        *o = pack8(px);
        n += 1;
    }
    n
}

/// `pack8` の結果から面 `plane` のバイトを取り出す
pub fn plane_byte(packed: u32, plane: usize) -> u8 {
    (packed >> (8 * plane)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 面のバイトから 8 ピクセルの色番号に戻す（テスト用）
    fn unpack(packed: u32) -> [u8; 8] {
        core::array::from_fn(|i| {
            (0..4).fold(0u8, |c, p| c | ((plane_byte(packed, p) >> (7 - i) & 1) << p))
        })
    }

    #[test]
    fn single_pixel_bits() {
        // 左端の白（15）は全部の面の最上位ビット
        assert_eq!(pack8(&[15, 0, 0, 0, 0, 0, 0, 0]), 0x8080_8080);
        // 右端の青（1）は面 0 の最下位ビットだけ
        assert_eq!(pack8(&[0, 0, 0, 0, 0, 0, 0, 1]), 0x0000_0001);
        // 2 番目の赤（4）は面 2 のビット 6
        assert_eq!(pack8(&[0, 4]), 0x0040_0000);
    }

    #[test]
    fn every_color_in_every_position() {
        for c in 0..16u8 {
            for pos in 0..8 {
                let mut px = [0u8; 8];
                px[pos] = c;
                assert_eq!(unpack(pack8(&px)), px, "color {c} at {pos}");
            }
        }
    }

    #[test]
    fn mixed_row_round_trips() {
        let px = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(unpack(pack8(&px)), px);
        let px = [15, 14, 13, 12, 11, 10, 9, 0];
        assert_eq!(unpack(pack8(&px)), px);
    }

    #[test]
    fn upper_bits_are_ignored() {
        assert_eq!(pack8(&[0x1F; 8]), pack8(&[0x0F; 8]));
        assert_eq!(pack8(&[0xF0; 8]), 0);
    }

    #[test]
    fn short_tail_is_padded_with_black() {
        // 8 の倍数でない行の最後は、足りない所を 0 番の色にする
        let row = [9u8; 11];
        let mut out = [0xFFFF_FFFFu32; 4];
        assert_eq!(pack_row(&row, &mut out), 2);
        assert_eq!(unpack(out[0]), [9; 8]);
        assert_eq!(unpack(out[1]), [9, 9, 9, 0, 0, 0, 0, 0]);
        assert_eq!(out[2], 0xFFFF_FFFF);
    }

    #[test]
    fn pack_row_stops_at_output_length() {
        let row = [3u8; 640];
        let mut out = [0u32; 10];
        assert_eq!(pack_row(&row, &mut out), 10);
        let mut full = [0u32; 80];
        assert_eq!(pack_row(&row, &mut full), 80);
        assert!(full.iter().all(|&v| v == pack8(&[3; 8])));
    }
}
