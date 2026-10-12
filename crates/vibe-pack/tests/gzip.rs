//! gzip の展開のテスト。gzip はここで組み立てる（ホストの gzip コマンドには頼らない）

use miniz_oxide::deflate::compress_to_vec;
use vibe_lzsa::samples::{Rng, mixed};
use vibe_pack::crc32;
use vibe_pack::gzip::{GzipError, gunzip, is_gzip};

const MAX: usize = 1 << 24;

/// `flg` に応じた欄を付けた gzip を作る
fn gzip_with(data: &[u8], flg: u8) -> Vec<u8> {
    let mut g = vec![0x1F, 0x8B, 8, flg, 0x12, 0x34, 0x56, 0x78, 2, 3];
    if flg & 0x04 != 0 {
        g.extend_from_slice(&[4, 0, b'A', b'B', 2, 0]); // FEXTRA: 4 バイト
    }
    if flg & 0x08 != 0 {
        g.extend_from_slice(b"HELLO.ELF\0");
    }
    if flg & 0x10 != 0 {
        g.extend_from_slice(b"a comment\0");
    }
    if flg & 0x02 != 0 {
        let c = crc32(&g) as u16;
        g.extend_from_slice(&c.to_le_bytes());
    }
    g.extend(compress_to_vec(data, 9));
    g.extend_from_slice(&crc32(data).to_le_bytes());
    g.extend_from_slice(&(data.len() as u32).to_le_bytes());
    g
}

fn gzip(data: &[u8]) -> Vec<u8> {
    gzip_with(data, 0)
}

#[test]
fn roundtrip_sizes_and_header_fields() {
    for (seed, size) in [(0, 0), (1, 1), (2, 100), (3, 40_000), (4, 300_000)] {
        let data = mixed(seed, size);
        for flg in [0, 0x02, 0x04, 0x08, 0x10, 0x1E] {
            let g = gzip_with(&data, flg);
            assert!(is_gzip(&g));
            assert_eq!(gunzip(&g, MAX).unwrap(), data, "size {size} flg {flg:x}");
        }
    }
    // 圧縮しないブロック（level 0）
    let data = Rng::new(7).bytes(5000);
    let mut g = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
    g.extend(compress_to_vec(&data, 0));
    g.extend_from_slice(&crc32(&data).to_le_bytes());
    g.extend_from_slice(&(data.len() as u32).to_le_bytes());
    assert_eq!(gunzip(&g, MAX).unwrap(), data);
}

#[test]
fn errors() {
    let data = mixed(9, 20_000);
    let g = gzip(&data);
    assert_eq!(gunzip(b"\x7fELF", MAX), Err(GzipError::NotGzip));
    assert_eq!(gunzip(&g[..9], MAX), Err(GzipError::BadHeader));
    let mut b = g.clone();
    b[2] = 7; // deflate 以外
    assert_eq!(gunzip(&b, MAX), Err(GzipError::BadHeader));
    let mut b = g.clone();
    b[3] = 0x20; // 予約のフラグ
    assert_eq!(gunzip(&b, MAX), Err(GzipError::BadHeader));
    let mut b = gzip_with(&data, 0x0A);
    b[10] ^= 1; // FHCRC が合わない
    assert_eq!(gunzip(&b, MAX), Err(GzipError::BadHeader));
    let n = g.len();
    let mut b = g.clone();
    b[n - 8] ^= 1; // CRC32
    assert_eq!(gunzip(&b, MAX), Err(GzipError::BadCrc));
    let mut b = g.clone();
    b[n - 4] ^= 1; // ISIZE
    assert_eq!(gunzip(&b, MAX), Err(GzipError::SizeMismatch));
    assert_eq!(gunzip(&g, data.len() - 1), Err(GzipError::TooLarge));
    let mut b = g.clone();
    b[n - 1] = 0xFF; // 4GB 近い ISIZE でメモリを取りに行かない
    assert_eq!(gunzip(&b, MAX), Err(GzipError::TooLarge));
    // deflate の後ろに余分なデータ
    let mut b = g[..n - 8].to_vec();
    b.extend_from_slice(b"junk");
    b.extend_from_slice(&g[n - 8..]);
    assert_eq!(gunzip(&b, MAX), Err(GzipError::SizeMismatch));
}

/// どこを壊しても・どこで切っても panic しない
#[test]
fn corrupt_input_does_not_panic() {
    let mut r = Rng::new(11);
    for seed in 0..5 {
        let g = gzip_with(&mixed(seed, 10_000), 0x1E);
        for cut in 0..g.len() {
            assert!(gunzip(&g[..cut], MAX).is_err());
        }
        for _ in 0..500 {
            let mut b = g.clone();
            let i = r.below(b.len());
            b[i] ^= 1 + r.below(255) as u8;
            let _ = gunzip(&b, MAX);
        }
    }
}
