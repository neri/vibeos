//! 圧縮したカーネルのファイルの形（docs/plans/compress.md §3.4）
//!
//! ```text
//! 0   4  印 "VPK1"
//! 4   1  形式（2 = LZSA2）
//! 5   1  フラグ（bit0 = BCJ）
//! 6   2  ヘッダの大きさ（後で欄を足しても読み飛ばせるように）
//! 8   4  展開後の大きさ
//! 12  4  BCJ の基準の番地（展開後のデータが置かれる番地）
//! 16  4  展開後の中身の CRC32
//! 20     LZSA2 のストリーム
//! ```
//!
//! ローダ（boot/loader.asm）も同じ形を読む。変えるときは両方を直すこと。

#![cfg_attr(not(test), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub const MAGIC: [u8; 4] = *b"VPK1";
pub const HEADER_SIZE: usize = 20;
pub const FORMAT_LZSA2: u8 = 2;
pub const FLAG_BCJ: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    /// 先頭が "VPK1" でない（圧縮していないファイル）
    NotPacked,
    /// ヘッダが壊れている・知らない形式やフラグ
    BadHeader,
    /// LZSA の展開に失敗した
    Lzsa(vibe_lzsa::Error),
    /// 展開した大きさがヘッダと違う
    SizeMismatch,
    /// 展開した中身の CRC32 がヘッダと違う
    BadCrc,
    /// 出力先が足りない
    OutputFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format: u8,
    pub flags: u8,
    pub header_size: u16,
    pub size: u32,
    pub bcj_base: u32,
    pub crc: u32,
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl Header {
    /// ヘッダを読んで確かめる
    pub fn parse(b: &[u8]) -> Result<Header, PackError> {
        if !is_packed(b) {
            return Err(PackError::NotPacked);
        }
        if b.len() < HEADER_SIZE {
            return Err(PackError::BadHeader);
        }
        let h = Header {
            format: b[4],
            flags: b[5],
            header_size: u16::from_le_bytes([b[6], b[7]]),
            size: u32_at(b, 8),
            bcj_base: u32_at(b, 12),
            crc: u32_at(b, 16),
        };
        let hs = h.header_size as usize;
        if h.format != FORMAT_LZSA2 || h.flags & !FLAG_BCJ != 0 || hs < HEADER_SIZE || hs > b.len() {
            return Err(PackError::BadHeader);
        }
        Ok(h)
    }

    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0; HEADER_SIZE];
        b[..4].copy_from_slice(&MAGIC);
        b[4] = self.format;
        b[5] = self.flags;
        b[6..8].copy_from_slice(&self.header_size.to_le_bytes());
        b[8..12].copy_from_slice(&self.size.to_le_bytes());
        b[12..16].copy_from_slice(&self.bcj_base.to_le_bytes());
        b[16..20].copy_from_slice(&self.crc.to_le_bytes());
        b
    }
}

/// 先頭が "VPK1" か
pub fn is_packed(b: &[u8]) -> bool {
    b.starts_with(&MAGIC)
}

/// CRC32（zlib・PNG と同じ。多項式 0xEDB88320、初期値と最後の反転あり）
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// x86 の BCJ フィルタ。`call`（E8）・`jmp`（E9）の後ろの相対番地を絶対番地に直す
/// （`encode`）／戻す。同じ関数を呼ぶ命令が同じバイト列になり、一致が増える。
///
/// 相対番地の上位 1 バイトが 00 か FF のときだけ直し、結果を 25 ビットで符号を広げて
/// 上位 1 バイトを 00/FF に保つ。直したら 5 バイト進める。`base` は `buf[0]` が置かれる番地。
///
/// 戻す側が同じ位置で同じ判断をできるように、判断に使うバイト（E8/E9 と、その 4 バイト後）が
/// 後の書き換えで変わらないようにする: 直さなかった E8/E9 が位置 j にあれば、j + 4 より前から
/// 始まる書き換えはしない（それをすると j + 4 のバイトが変わる）
fn bcj(buf: &mut [u8], base: u32, encode: bool) {
    let mut i = 0;
    let mut blocked_until = 0;
    while i + 5 <= buf.len() {
        if buf[i] & 0xFE == 0xE8 {
            if i >= blocked_until && (buf[i + 4] == 0 || buf[i + 4] == 0xFF) {
                let v = u32_at(buf, i + 1);
                let here = base.wrapping_add(i as u32 + 5);
                let v = if encode { v.wrapping_add(here) } else { v.wrapping_sub(here) };
                let v = ((v << 7) as i32 >> 7) as u32;
                buf[i + 1..i + 5].copy_from_slice(&v.to_le_bytes());
                i += 5;
                continue;
            }
            blocked_until = i + 4;
        }
        i += 1;
    }
}

pub fn bcj_encode(buf: &mut [u8], base: u32) {
    bcj(buf, base, true);
}

pub fn bcj_decode(buf: &mut [u8], base: u32) {
    bcj(buf, base, false);
}

/// 展開する。`out` は展開後の大きさ以上であること。展開した大きさを返す
pub fn unpack(src: &[u8], out: &mut [u8]) -> Result<usize, PackError> {
    let h = Header::parse(src)?;
    let size = h.size as usize;
    if out.len() < size {
        return Err(PackError::OutputFull);
    }
    let out = &mut out[..size];
    let n = vibe_lzsa::decompress(&src[h.header_size as usize..], out).map_err(|e| match e {
        // ヘッダの大きさより長く展開しようとした
        vibe_lzsa::Error::OutputFull => PackError::SizeMismatch,
        e => PackError::Lzsa(e),
    })?;
    if n != size {
        return Err(PackError::SizeMismatch);
    }
    if h.flags & FLAG_BCJ != 0 {
        bcj_decode(out, h.bcj_base);
    }
    if crc32(out) != h.crc {
        return Err(PackError::BadCrc);
    }
    Ok(size)
}

/// 展開して `Vec` で返す
#[cfg(feature = "alloc")]
pub fn unpack_to_vec(src: &[u8]) -> Result<alloc::vec::Vec<u8>, PackError> {
    let h = Header::parse(src)?;
    let mut out = alloc::vec![0; h.size as usize];
    unpack(src, &mut out)?;
    Ok(out)
}

/// LZSA2 で圧縮する。`bcj_base` があれば先に BCJ をかける（展開後に置かれる番地）
#[cfg(feature = "alloc")]
pub fn pack(data: &[u8], bcj_base: Option<u32>) -> alloc::vec::Vec<u8> {
    let mut body = data.to_vec();
    if let Some(base) = bcj_base {
        bcj_encode(&mut body, base);
    }
    let h = Header {
        format: FORMAT_LZSA2,
        flags: if bcj_base.is_some() { FLAG_BCJ } else { 0 },
        header_size: HEADER_SIZE as u16,
        size: data.len() as u32,
        bcj_base: bcj_base.unwrap_or(0),
        crc: crc32(data),
    };
    let opt = vibe_lzsa::Options::new(vibe_lzsa::Version::V2);
    let stream = vibe_lzsa::compress(&body, &opt).expect("stream format has no size limit");
    let mut out = h.to_bytes().to_vec();
    out.extend_from_slice(&stream);
    out
}
