use vibe_lzsa::samples::{Rng, mixed};
use vibe_pack::*;

#[test]
fn crc32_known_values() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
}

fn bcj_roundtrip(data: &[u8], base: u32) {
    let mut b = data.to_vec();
    bcj_encode(&mut b, base);
    bcj_decode(&mut b, base);
    assert_eq!(b, data);
}

#[test]
fn bcj_roundtrips() {
    let mut r = Rng::new(3);
    for n in [0, 1, 4, 5, 6, 9, 10, 100, 5000] {
        bcj_roundtrip(&r.bytes(n), 0x100000);
    }
    // E8・E9 と 00/FF だらけのデータ（判断が分かれる所が多い）
    for seed in 0..50 {
        let mut r = Rng::new(seed);
        let d: Vec<u8> = (0..3000).map(|_| [0xE8, 0xE9, 0x00, 0xFF, 0x12][r.below(5)]).collect();
        for base in [0, 0x100000, 0xFFFF_FF00] {
            bcj_roundtrip(&d, base);
        }
    }
    // ファイルの終わりの 4 バイト以内の E8 は直さない
    let mut d = vec![0x90, 0x90, 0xE8, 0x01, 0x00, 0x00];
    let orig = d.clone();
    bcj_encode(&mut d, 0);
    assert_eq!(d, orig);
}

#[test]
fn bcj_makes_calls_absolute() {
    // 0x100000 + 0x10 の call で 0x100100 を呼ぶ: 相対は 0x100100 - 0x100015 = 0xEB
    let mut d = vec![0u8; 0x20];
    d[0x10] = 0xE8;
    d[0x11..0x15].copy_from_slice(&0xEBu32.to_le_bytes());
    bcj_encode(&mut d, 0x100000);
    assert_eq!(&d[0x11..0x15], &0x100100u32.to_le_bytes());
    // 後ろ向きの call（相対が負）
    let mut d = vec![0u8; 0x20];
    d[0x10] = 0xE8;
    d[0x11..0x15].copy_from_slice(&(-0x15i32 as u32).to_le_bytes());
    bcj_encode(&mut d, 0x100000);
    assert_eq!(&d[0x11..0x15], &0x100000u32.to_le_bytes());
}

#[test]
fn pack_unpack_roundtrip() {
    for (seed, size) in [(0, 0), (1, 1), (2, 1000), (3, 70_000), (4, 200_000)] {
        let data = mixed(seed, size);
        for base in [None, Some(0x100000)] {
            let packed = pack(&data, base);
            assert!(is_packed(&packed));
            assert_eq!(unpack_to_vec(&packed).unwrap(), data);
            let h = Header::parse(&packed).unwrap();
            assert_eq!(h.size as usize, data.len());
            assert_eq!(h.flags & FLAG_BCJ != 0, base.is_some());
        }
    }
}

#[test]
fn broken_files_are_errors() {
    let data = mixed(9, 20_000);
    let packed = pack(&data, Some(0x100000));
    assert_eq!(unpack_to_vec(b"hello").err(), Some(PackError::NotPacked));
    assert_eq!(unpack_to_vec(&packed[..10]).err(), Some(PackError::BadHeader));
    let mut b = packed.clone();
    b[4] = 1; // 知らない形式
    assert_eq!(unpack_to_vec(&b).err(), Some(PackError::BadHeader));
    let mut b = packed.clone();
    b[16] ^= 1; // CRC
    assert_eq!(unpack_to_vec(&b).err(), Some(PackError::BadCrc));
    let mut b = packed.clone();
    b[8] ^= 1; // 大きさ
    assert!(matches!(unpack_to_vec(&b).err(), Some(PackError::SizeMismatch)));
    let mut out = vec![0; data.len() - 1];
    assert_eq!(unpack(&packed, &mut out).err(), Some(PackError::OutputFull));
    // どこを壊しても panic しない（たいていは CRC で気づく）
    let mut r = Rng::new(1);
    for _ in 0..500 {
        let mut b = packed.clone();
        let i = HEADER_SIZE + r.below(b.len() - HEADER_SIZE);
        b[i] ^= 1 + r.below(255) as u8;
        assert!(unpack_to_vec(&b).is_err());
    }
    for cut in (0..packed.len()).step_by(97) {
        assert!(unpack_to_vec(&packed[..cut]).is_err());
    }
}
