//! コマンドの並びをブロックのバイト列にする。各欄の大きさ（ビット数）もここで決める

use crate::Version;
use crate::format::*;
use alloc::vec::Vec;

/// 1 つのコマンド: リテラル `lits` バイト（入力の `lit_start` から）のあとに、`dist` バイト前から
/// `len` バイトの一致。`len == 0` はブロックの最後のリテラルだけのコマンド
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cmd {
    pub lit_start: usize,
    pub lits: usize,
    pub len: usize,
    pub dist: usize,
}

pub fn min_match(v: Version) -> usize {
    match v {
        Version::V1 => MIN_MATCH_V1,
        Version::V2 => MIN_MATCH_V2,
    }
}

/// リテラルの数の、トークンの外に書く分のビット数
pub fn lit_ext_bits(v: Version, n: usize) -> u32 {
    match v {
        Version::V1 => match n {
            0..7 => 0,
            7..256 => 8,
            256..512 => 16,
            _ => 24,
        },
        Version::V2 => match n {
            0..3 => 0,
            3..18 => 4,
            18..256 => 12,
            _ => 28,
        },
    }
}

/// 一致の長さの、トークンの外に書く分のビット数
pub fn match_ext_bits(v: Version, len: usize) -> u32 {
    match v {
        Version::V1 => match len {
            0..18 => 0,
            18..256 => 8,
            256..512 => 16,
            _ => 24,
        },
        Version::V2 => match len {
            0..9 => 0,
            9..24 => 4,
            24..256 => 12,
            _ => 28,
        },
    }
}

/// オフセットのビット数（`rep`: 前回と同じオフセットを LZSA2 の rep-match で書ける）
pub fn offset_bits(v: Version, dist: usize, rep: bool) -> u32 {
    match v {
        Version::V1 => {
            if dist <= 256 {
                8
            } else {
                16
            }
        }
        Version::V2 => match dist {
            _ if rep => 0,
            0..=32 => 4,
            33..=512 => 8,
            513..=8704 => 12,
            _ => 16,
        },
    }
}

/// 1 ブロック分を書く。LZSA2 のニブルは、最初のニブルを書くときに 1 バイト確保して上位 4 ビットに
/// 入れ、次のニブルをその下位 4 ビットに入れる（展開器が読む順と同じになる）
struct Writer<'a> {
    out: &'a mut Vec<u8>,
    nibble_at: Option<usize>,
}

impl Writer<'_> {
    fn byte(&mut self, b: u8) {
        self.out.push(b);
    }

    fn u16(&mut self, v: usize) {
        self.out.push(v as u8);
        self.out.push((v >> 8) as u8);
    }

    fn nibble(&mut self, n: usize) {
        match self.nibble_at.take() {
            Some(i) => self.out[i] |= n as u8 & 0x0f,
            None => {
                self.nibble_at = Some(self.out.len());
                self.out.push((n as u8) << 4);
            }
        }
    }

    fn v1_lits(&mut self, n: usize) {
        match n {
            0..7 => {}
            7..256 => self.byte((n - V1_LIT_RUN) as u8),
            256..512 => {
                self.byte(250);
                self.byte((n - 256) as u8);
            }
            _ => {
                self.byte(249);
                self.u16(n);
            }
        }
    }

    fn v1_match_len(&mut self, len: usize) {
        match len {
            0..18 => {}
            18..256 => self.byte((len - MIN_MATCH_V1 - V1_MATCH_RUN) as u8),
            256..512 => {
                self.byte(239);
                self.byte((len - 256) as u8);
            }
            _ => {
                self.byte(238);
                self.u16(len);
            }
        }
    }

    /// LZSA2 の長さの続き。`base` はトークンの欄が埋まったときの値
    fn v2_len(&mut self, n: usize, base: usize) {
        if n < base + 15 {
            self.nibble(n - base);
        } else if n < 256 {
            self.nibble(15);
            self.byte((n - base - 15) as u8);
        } else {
            self.nibble(15);
            self.byte((257 - base - 15) as u8);
            self.u16(n);
        }
    }

    fn literals(&mut self, data: &[u8], c: &Cmd) {
        self.out.extend_from_slice(&data[c.lit_start..c.lit_start + c.lits]);
    }
}

/// ブロックのコマンドを `out` に書く。`raw` なら最後に EOD の印を書く
pub fn write_block(v: Version, data: &[u8], cmds: &[Cmd], raw: bool, out: &mut Vec<u8>) {
    let mut w = Writer { out, nibble_at: None };
    let mut rep = 0;
    for c in cmds {
        let last = c.len == 0;
        match v {
            Version::V1 => {
                let l = c.lits.min(V1_LIT_RUN) as u8;
                if last {
                    w.byte(l << 4 | if raw { 0x0f } else { 0 });
                    w.v1_lits(c.lits);
                    w.literals(data, c);
                    if raw {
                        // 1 バイトのオフセット 0 と、2 バイトの長さ 0
                        w.byte(0);
                        w.byte(238);
                        w.u16(0);
                    }
                    break;
                }
                let m = (c.len - MIN_MATCH_V1).min(V1_MATCH_RUN) as u8;
                let long = c.dist > 256;
                w.byte(if long { V1_LONG_OFFSET } else { 0 } | l << 4 | m);
                w.v1_lits(c.lits);
                w.literals(data, c);
                let o = (c.dist - 1) ^ 0xffff;
                w.byte(o as u8);
                if long {
                    w.byte((o >> 8) as u8);
                }
                w.v1_match_len(c.len);
            }
            Version::V2 => {
                let l = (c.lits.min(V2_LIT_RUN) as u8) << 3;
                if last {
                    // raw は rep-match + 長さの EOD（本家と同じ。オフセットの欄が要らない）
                    w.byte(l | if raw { V2_REP | 7 } else { 0 });
                    if c.lits >= V2_LIT_RUN {
                        w.v2_len(c.lits, V2_LIT_RUN);
                    }
                    w.literals(data, c);
                    if raw {
                        w.nibble(15);
                        w.byte(232);
                    }
                    break;
                }
                let m = (c.len - MIN_MATCH_V2).min(V2_MATCH_RUN) as u8;
                let d = c.dist;
                // オフセットの形（トークンの XYZ）と、その後に書くニブル・バイト（書く順）
                let (xyz, nib, bytes, nbytes) = if d == rep {
                    (V2_REP, None, [0, 0], 0)
                } else if d <= 32 {
                    let x = (d - 1) ^ 0x1e;
                    (((x & 1) as u8) << 5, Some(x >> 1), [0, 0], 0)
                } else if d <= 512 {
                    let x = (d - 1) ^ 0xff;
                    (0x40 | ((x >> 8 & 1) as u8) << 5, None, [x as u8, 0], 1)
                } else if d <= 8704 {
                    let x = (d - 513) ^ 0x1eff;
                    (0x80 | ((x >> 8 & 1) as u8) << 5, Some(x >> 9), [x as u8, 0], 1)
                } else {
                    let x = (d - 1) ^ 0xffff;
                    (0xc0, None, [(x >> 8) as u8, x as u8], 2)
                };
                w.byte(xyz | l | m);
                if c.lits >= V2_LIT_RUN {
                    w.v2_len(c.lits, V2_LIT_RUN);
                }
                w.literals(data, c);
                if let Some(n) = nib {
                    w.nibble(n);
                }
                for &b in &bytes[..nbytes] {
                    w.byte(b);
                }
                if c.len >= MIN_MATCH_V2 + V2_MATCH_RUN {
                    w.v2_len(c.len, MIN_MATCH_V2 + V2_MATCH_RUN);
                }
                rep = d;
            }
        }
    }
}
