//! 自作の圧縮器と展開器だけで確かめるテスト（本家のツールには頼らない）

use vibe_lzsa::samples::{Rng, mixed, samples};
use vibe_lzsa::*;

const PARSERS: [Parser; 4] = [Parser::Greedy, Parser::Lazy, Parser::Optimal, Parser::OptimalRep];

fn roundtrip(name: &str, data: &[u8], opt: &Options) {
    let packed = match compress(data, opt) {
        Ok(p) => p,
        // raw block に収まらない（縮まないデータ）
        Err(CompressError::TooLarge) if opt.raw => return,
        Err(e) => panic!("{name} {opt:?}: {e:?}"),
    };
    let got = if opt.raw {
        decompress_raw_to_vec(opt.version, &packed)
    } else {
        decompress_to_vec(&packed)
    };
    assert_eq!(got.as_deref(), Ok(data), "{name} {opt:?}");
    // 大きさの分かっている出力先へ（ヒープを使わない方の関数）
    let mut out = vec![0; data.len()];
    let n = if opt.raw {
        decompress_raw(opt.version, &packed, &mut out)
    } else {
        decompress(&packed, &mut out)
    };
    assert_eq!(n, Ok(data.len()), "{name} {opt:?}");
    assert_eq!(out, data, "{name} {opt:?}");
}

#[test]
fn all_samples_all_settings() {
    for (name, data) in samples() {
        for version in [Version::V1, Version::V2] {
            for parser in PARSERS {
                for raw in [false, true] {
                    if raw && data.len() > BLOCK_SIZE {
                        continue;
                    }
                    let opt = Options { raw, parser, ..Options::new(version) };
                    roundtrip(&name, &data, &opt);
                }
            }
        }
    }
}

#[test]
fn few_arrivals_and_shallow_search() {
    for seed in 0..20 {
        let data = mixed(1000 + seed, 5000);
        for (depth, arrivals, extra) in [(1, 1, 0), (4, 2, 1), (1024, 16, 64)] {
            let opt = Options { depth, arrivals, extra, ..Options::new(Version::V2) };
            roundtrip("mixed", &data, &opt);
        }
    }
}

#[test]
fn output_too_small_is_an_error() {
    let data = mixed(7, 3000);
    for version in [Version::V1, Version::V2] {
        let packed = compress(&data, &Options::new(version)).unwrap();
        let mut out = vec![0; data.len() - 1];
        assert_eq!(decompress(&packed, &mut out), Err(Error::OutputFull));
    }
}

/// 壊れたデータ（途中で切る・バイトを書き換える）で panic しない
#[test]
fn corrupt_input_does_not_panic() {
    let mut r = Rng::new(99);
    for seed in 0..10 {
        let data = mixed(seed, 2000);
        for version in [Version::V1, Version::V2] {
            for raw in [false, true] {
                let packed = compress(&data, &Options { raw, ..Options::new(version) }).unwrap();
                let mut out = vec![0; 4096];
                let run = |src: &[u8], out: &mut [u8]| {
                    if raw {
                        let _ = decompress_raw(version, src, out);
                        let _ = decompress_raw_to_vec(version, src);
                    } else {
                        let _ = decompress(src, out);
                        let _ = decompress_to_vec(src);
                    }
                };
                for cut in 0..packed.len() {
                    run(&packed[..cut], &mut out);
                }
                for _ in 0..300 {
                    let mut bad = packed.clone();
                    let i = r.below(bad.len());
                    bad[i] = r.next_u32() as u8;
                    run(&bad, &mut out);
                }
            }
        }
    }
    for _ in 0..2000 {
        let n = r.below(64);
        let junk = r.bytes(n);
        let mut out = vec![0; 256];
        for version in [Version::V1, Version::V2] {
            let _ = decompress_raw(version, &junk, &mut out);
            let _ = decompress_block(version, &junk, &mut out, 0, false);
        }
        let mut s = vec![0x7b, 0x9e, if n.is_multiple_of(2) { 0x00 } else { 0x20 }];
        s.extend_from_slice(&junk);
        let _ = decompress(&s, &mut out);
        let _ = decompress_to_vec(&s);
    }
}

/// 仕様書から手で作ったデータを展開する
#[test]
fn hand_made_streams() {
    // LZSA1 raw: リテラル 'a' + 1 バイト前から 9 バイト、EOD
    let v1 = [0x16, b'a', 0xff, 0x0f, 0x00, 0xee, 0x00, 0x00];
    assert_eq!(decompress_raw_to_vec(Version::V1, &v1).unwrap(), b"aaaaaaaaaa");
    // LZSA2 のストリーム: リテラル "abc"（長さはニブル 0）+ 3 バイト前（5 ビットのオフセット）から
    // 6 バイト。ニブルは 1 バイトの上位と下位に入る
    let v2 = [0x7b, 0x9e, 0x20, 6, 0, 0, 0x1c, 0x0e, b'a', b'b', b'c', 0x00, 0, 0, 0];
    assert_eq!(decompress_to_vec(&v2).unwrap(), b"abcabcabc");
    // 無圧縮のフレーム
    let raw_frame = [0x7b, 0x9e, 0x00, 3, 0, 0x80, b'x', b'y', b'z', 0, 0, 0];
    assert_eq!(decompress_to_vec(&raw_frame).unwrap(), b"xyz");
}

#[test]
fn raw_limits() {
    let mut r = Rng::new(5);
    // 縮まない 64KB は raw block に入らない。64KB を超える入力も
    assert_eq!(compress(&r.bytes(BLOCK_SIZE), &Options { raw: true, ..Options::new(Version::V2) }), Err(CompressError::TooLarge));
    assert_eq!(compress(&vec![0; BLOCK_SIZE + 1], &Options { raw: true, ..Options::new(Version::V1) }), Err(CompressError::TooLarge));
    // 縮まない小さいデータは入る
    let small = r.bytes(1000);
    roundtrip("small-random", &small, &Options { raw: true, ..Options::new(Version::V1) });
    // 空の raw block は空
    assert_eq!(decompress_raw_to_vec(Version::V2, &[]).unwrap(), b"");
}
