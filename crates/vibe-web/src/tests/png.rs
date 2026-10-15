use crate::png::{Image, PngError, decode};
use miniz_oxide::deflate::compress_to_vec_zlib;

const WHITE: u32 = 0xFF_FFFF;

fn rgb(x: usize, y: usize) -> [u8; 3] {
    [(x * 40 % 256) as u8, (y * 50 % 256) as u8, ((x + y) * 30 % 256) as u8]
}

fn rgb32(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// 地の色（白）の上にアルファで重ねる（png.rs と同じ丸め）
fn over_white(c: [u8; 3], a: u8) -> u32 {
    let ch = |v: u8| (v as u32 * a as u32 + 255 * (255 - a as u32) + 127) / 255;
    ch(c[0]) << 16 | ch(c[1]) << 8 | ch(c[2])
}

fn expect(w: usize, h: usize, f: impl Fn(usize, usize) -> u32) -> Image {
    Image { width: w, height: h, pixels: (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect() }
}

// --- ほかの実装（PIL）で作った PNG（testdata/make-png.py） ---

#[test]
fn pil_rgb8() {
    let img = decode(include_bytes!("../../testdata/RGB8.PNG"), 1 << 20, WHITE).unwrap();
    assert_eq!(img, expect(13, 7, |x, y| rgb32(rgb(x, y))));
}

#[test]
fn pil_rgba8() {
    let img = decode(include_bytes!("../../testdata/RGBA8.PNG"), 1 << 20, WHITE).unwrap();
    assert_eq!(img, expect(7, 6, |x, y| over_white(rgb(x, y), (x * 60 % 256) as u8)));
}

#[test]
fn pil_gray16() {
    let img = decode(include_bytes!("../../testdata/GRAY16.PNG"), 1 << 20, WHITE).unwrap();
    assert_eq!(
        img,
        expect(5, 3, |x, y| {
            let v = ((x * 13000 + y * 7000) % 65536 >> 8) as u8;
            rgb32([v, v, v])
        })
    );
}

#[test]
fn pil_gray_alpha8() {
    let img = decode(include_bytes!("../../testdata/GRAYA8.PNG"), 1 << 20, WHITE).unwrap();
    assert_eq!(
        img,
        expect(6, 4, |x, y| {
            let v = (x * 20) as u8;
            over_white([v, v, v], (y * 80) as u8)
        })
    );
}

#[test]
fn pil_gray1() {
    let img = decode(include_bytes!("../../testdata/GRAY1.PNG"), 1 << 20, WHITE).unwrap();
    assert_eq!(img, expect(11, 3, |x, y| if (x + y) % 3 == 0 { WHITE } else { 0 }));
}

#[test]
fn pil_palette4_with_transparency() {
    let img = decode(include_bytes!("../../testdata/PAL4.PNG"), 1 << 20, WHITE).unwrap();
    assert_eq!(
        img,
        expect(9, 5, |x, y| {
            let i = (x + 2 * y) % 16;
            let c = [(i * 16) as u8, (255 - i * 16) as u8, (i * 8) as u8];
            over_white(c, [0u8, 128].get(i).copied().unwrap_or(255))
        })
    );
}

// --- テストの中の符号化器（フィルタの全種類・Adam7・いろいろな色の種類と深さ） ---

/// 1 画素の成分（深さのビット数に収まる値）
type Sample = fn(usize, usize) -> Vec<u16>;

fn crc(data: &[u8]) -> u32 {
    vibe_pack::crc32(data)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let c = crc(&out[start..]);
    out.extend_from_slice(&c.to_be_bytes());
}

/// 成分を深さ `depth` で詰めた 1 行
fn pack(samples: &[u16], depth: u8) -> Vec<u8> {
    match depth {
        16 => samples.iter().flat_map(|s| s.to_be_bytes()).collect(),
        8 => samples.iter().map(|&s| s as u8).collect(),
        d => {
            let mut out = vec![0u8; (samples.len() * d as usize).div_ceil(8)];
            for (i, &s) in samples.iter().enumerate() {
                let bit = i * d as usize;
                out[bit / 8] |= (s as u8) << (8 - d as usize - bit % 8);
            }
            out
        }
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn filter(kind: u8, cur: &[u8], prev: &[u8], bpp: usize) -> Vec<u8> {
    let up = |i: usize| prev.get(i).copied().unwrap_or(0);
    (0..cur.len())
        .map(|i| {
            let a = if i >= bpp { cur[i - bpp] } else { 0 };
            let c = if i >= bpp { up(i - bpp) } else { 0 };
            let pred = match kind {
                0 => 0,
                1 => a,
                2 => up(i),
                3 => ((a as u16 + up(i) as u16) / 2) as u8,
                _ => paeth(a, up(i), c),
            };
            cur[i].wrapping_sub(pred)
        })
        .collect()
}

const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// PNG を作る。行のフィルタは行ごとに 0〜4 を順に使う。`extra` は IHDR の後ろに入れる塊（PLTE・tRNS）
#[allow(clippy::too_many_arguments)]
fn encode(w: usize, h: usize, depth: u8, color: u8, interlace: bool, sample: Sample, extra: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let channels = match color {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        _ => 4,
    };
    let bpp = (channels * depth as usize).div_ceil(8);
    let passes: Vec<(usize, usize, usize, usize)> = if interlace { ADAM7.to_vec() } else { vec![(0, 0, 1, 1)] };
    let mut raw = Vec::new();
    let mut n = 0u8;
    for (x0, y0, dx, dy) in passes {
        let xs: Vec<usize> = (x0..w).step_by(dx).collect();
        let ys: Vec<usize> = (y0..h).step_by(dy).collect();
        if xs.is_empty() || ys.is_empty() {
            continue;
        }
        let mut prev: Vec<u8> = Vec::new();
        for &y in &ys {
            let samples: Vec<u16> = xs.iter().flat_map(|&x| sample(x, y)).collect();
            let cur = pack(&samples, depth);
            let kind = n % 5;
            n += 1;
            raw.push(kind);
            raw.extend(filter(kind, &cur, &prev, bpp));
            prev = cur;
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[depth, color, 0, 0, interlace as u8]);
    chunk(&mut out, b"IHDR", &ihdr);
    for (kind, body) in extra {
        chunk(&mut out, kind, body);
    }
    let z = compress_to_vec_zlib(&raw, 6);
    // IDAT を 2 つに分ける（つなげて読むことを確かめる）
    let (a, b) = z.split_at(z.len() / 2);
    chunk(&mut out, b"IDAT", a);
    chunk(&mut out, b"IDAT", b);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn rgb_sample(x: usize, y: usize) -> Vec<u16> {
    rgb(x, y).iter().map(|&v| v as u16).collect()
}

#[test]
fn every_filter_and_adam7_rgb8() {
    for interlace in [false, true] {
        for (w, h) in [(1, 1), (3, 2), (8, 8), (9, 9), (13, 7), (31, 17)] {
            let png = encode(w, h, 8, 2, interlace, rgb_sample, &[]);
            let img = decode(&png, 1 << 20, WHITE).unwrap_or_else(|e| panic!("{w}x{h} {interlace}: {e:?}"));
            assert_eq!(img, expect(w, h, |x, y| rgb32(rgb(x, y))), "{w}x{h} interlace {interlace}");
        }
    }
}

#[test]
fn sixteen_bit_rgba_and_gray_alpha() {
    let rgba16: Sample = |x, y| {
        let c = rgb(x, y);
        vec![c[0] as u16 * 257, c[1] as u16 * 257 + 1, c[2] as u16 * 257, ((x * 60 % 256) * 257) as u16]
    };
    for interlace in [false, true] {
        let img = decode(&encode(10, 6, 16, 6, interlace, rgba16, &[]), 1 << 20, WHITE).unwrap();
        assert_eq!(img, expect(10, 6, |x, y| over_white(rgb(x, y), (x * 60 % 256) as u8)));
    }
    let ga16: Sample = |x, y| vec![(x * 4000) as u16, if y == 0 { 0 } else { 65535 }];
    let img = decode(&encode(5, 2, 16, 4, false, ga16, &[]), 1 << 20, WHITE).unwrap();
    assert_eq!(
        img,
        expect(5, 2, |x, y| {
            let v = ((x * 4000) >> 8) as u8;
            if y == 0 { WHITE } else { rgb32([v, v, v]) }
        })
    );
}

#[test]
fn low_bit_depths() {
    // グレー 2・4 ビット（0〜255 に広げる）
    let g2: Sample = |x, y| vec![((x + y) % 4) as u16];
    let img = decode(&encode(7, 3, 2, 0, true, g2, &[]), 1 << 20, WHITE).unwrap();
    assert_eq!(
        img,
        expect(7, 3, |x, y| {
            let v = ((x + y) % 4 * 85) as u8;
            rgb32([v, v, v])
        })
    );
    let g4: Sample = |x, _| vec![(x % 16) as u16];
    let img = decode(&encode(17, 2, 4, 0, false, g4, &[]), 1 << 20, WHITE).unwrap();
    assert_eq!(
        img,
        expect(17, 2, |x, _| {
            let v = (x % 16 * 17) as u8;
            rgb32([v, v, v])
        })
    );
    // パレット 2 ビット・1 ビット
    let plte: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9];
    let p2: Sample = |x, y| vec![((x * 3 + y) % 4) as u16];
    let img = decode(&encode(6, 4, 2, 3, true, p2, &[(b"PLTE", plte.clone())]), 1 << 20, WHITE).unwrap();
    let pal = [0xFF0000, 0x00FF00, 0x0000FF, 0x090909];
    assert_eq!(img, expect(6, 4, |x, y| pal[(x * 3 + y) % 4]));
    let p1: Sample = |x, _| vec![(x % 2) as u16];
    let img = decode(&encode(9, 1, 1, 3, false, p1, &[(b"PLTE", plte)]), 1 << 20, WHITE).unwrap();
    assert_eq!(img, expect(9, 1, |x, _| pal[x % 2]));
}

#[test]
fn trns_single_color() {
    // RGB の tRNS: その色だけ透明（地の色になる）
    let s: Sample = |x, _| if x == 1 { vec![10, 20, 30] } else { vec![1, 2, 3] };
    let trns = vec![0, 10, 0, 20, 0, 30];
    let img = decode(&encode(3, 1, 8, 2, false, s, &[(b"tRNS", trns)]), 1 << 20, 0x123456).unwrap();
    assert_eq!(img.pixels, [0x010203, 0x123456, 0x010203]);
    // グレーの tRNS
    let s: Sample = |x, _| vec![x as u16 * 100];
    let img = decode(&encode(2, 1, 8, 0, false, s, &[(b"tRNS", vec![0, 100])]), 1 << 20, 0xABCDEF).unwrap();
    assert_eq!(img.pixels, [0, 0xABCDEF]);
}

#[test]
fn broken_files() {
    let good = encode(4, 4, 8, 2, false, rgb_sample, &[]);
    assert_eq!(decode(b"GIF89a", 100, 0), Err(PngError::NotPng));
    // CRC を壊す
    let mut bad = good.clone();
    bad[8 + 8 + 13] ^= 1;
    assert_eq!(decode(&bad, 100, 0), Err(PngError::BadCrc));
    // 大きすぎる
    assert_eq!(decode(&good, 15, 0), Err(PngError::TooLarge));
    // 途中で切れたもの（どこで切っても panic しない）
    for n in 0..good.len() {
        assert!(decode(&good[..n], 100, 0).is_err(), "cut at {n}");
    }
    // フィルタの種類が 5 以上
    let mut raw = vec![7u8];
    raw.extend_from_slice(&[0; 3]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    chunk(&mut png, b"IDAT", &compress_to_vec_zlib(&raw, 6));
    chunk(&mut png, b"IEND", &[]);
    assert_eq!(decode(&png, 100, 0), Err(PngError::BadFilter));
    // パレットの外の番号・パレットが無い
    let p: Sample = |_, _| vec![5];
    assert_eq!(decode(&encode(1, 1, 8, 3, false, p, &[(b"PLTE", vec![0, 0, 0])]), 100, 0), Err(PngError::BadPalette));
    assert_eq!(decode(&encode(1, 1, 8, 3, false, p, &[]), 100, 0), Err(PngError::BadPalette));
    // 扱えない深さ
    assert_eq!(decode(&encode(1, 1, 4, 2, false, |_, _| vec![1, 1, 1], &[]), 100, 0), Err(PngError::BadHeader));
}

#[test]
fn random_damage_does_not_panic() {
    let good = encode(16, 9, 8, 6, true, |x, y| vec![x as u16 * 9, y as u16 * 20, 7, (x * y) as u16], &[]);
    let mut seed = 0x9E37_79B9u32;
    for _ in 0..3000 {
        let mut b = good.clone();
        for _ in 0..1 + seed % 4 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let i = 8 + seed as usize % (b.len() - 8);
            b[i] = (seed >> 8) as u8;
        }
        // 中身を壊しても CRC を合わせ直して、展開・フィルタ・画素の読み方まで届くようにする
        let mut pos = 8;
        while pos + 12 <= b.len() {
            let len = u32::from_be_bytes([b[pos], b[pos + 1], b[pos + 2], b[pos + 3]]) as usize;
            let Some(end) = pos.checked_add(8 + len).filter(|&e| e + 4 <= b.len()) else { break };
            let c = crc(&b[pos + 4..end]);
            b[end..end + 4].copy_from_slice(&c.to_be_bytes());
            pos = end + 4;
        }
        let _ = decode(&b, 1 << 16, 0);
    }
}
