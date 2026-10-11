//! PS/2 マウスの 3 バイトのパケットの解釈

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MousePacket {
    /// 右が正
    pub dx: i16,
    /// 画面の下向きが正（PS/2 は上向きが正なので反転済み）
    pub dy: i16,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

#[derive(Default)]
pub struct MouseDecoder {
    buf: [u8; 3],
    len: usize,
}

const ALWAYS_ONE: u8 = 0x08;
const X_SIGN: u8 = 0x10;
const Y_SIGN: u8 = 0x20;
const X_OVERFLOW: u8 = 0x40;
const Y_OVERFLOW: u8 = 0x80;

impl MouseDecoder {
    pub const fn new() -> Self {
        MouseDecoder { buf: [0; 3], len: 0 }
    }

    /// 1 バイト受け取る。パケットがそろったら返す
    pub fn feed(&mut self, b: u8) -> Option<MousePacket> {
        // 先頭バイトは bit3 が必ず 1。そうでなければ途中から読んでいるので捨てて合わせ直す
        if self.len == 0 && b & ALWAYS_ONE == 0 {
            return None;
        }
        self.buf[self.len] = b;
        self.len += 1;
        if self.len < 3 {
            return None;
        }
        self.len = 0;
        let [flags, x, y] = self.buf;
        if flags & (X_OVERFLOW | Y_OVERFLOW) != 0 {
            return None; // あふれた値は信用できない
        }
        let dx = x as i16 - if flags & X_SIGN != 0 { 256 } else { 0 };
        let dy = y as i16 - if flags & Y_SIGN != 0 { 256 } else { 0 };
        Some(MousePacket { dx, dy: -dy, left: flags & 1 != 0, right: flags & 2 != 0, middle: flags & 4 != 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(bytes: &[u8]) -> Vec<MousePacket> {
        let mut d = MouseDecoder::new();
        bytes.iter().filter_map(|&b| d.feed(b)).collect()
    }

    #[test]
    fn movement_and_buttons() {
        // 右に 5、上に 3（PS/2 の y は上向きが正）、左ボタン
        let p = feed_all(&[0x09, 5, 3]);
        assert_eq!(p, [MousePacket { dx: 5, dy: -3, left: true, right: false, middle: false }]);
        // 左に 2、下に 4（符号ビット）、右ボタン
        let p = feed_all(&[0x08 | 0x10 | 0x20 | 0x02, 254, 252]);
        assert_eq!(p, [MousePacket { dx: -2, dy: 4, left: false, right: true, middle: false }]);
    }

    #[test]
    fn resyncs_on_bad_first_byte() {
        // 先頭の 0x05（bit3 が 0）は捨て、続く 3 バイトを 1 つのパケットとして読む
        let p = feed_all(&[0x05, 0x08, 1, 1]);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].dx, p[0].dy), (1, -1));
    }

    #[test]
    fn overflow_is_dropped() {
        assert!(feed_all(&[0x48, 255, 0]).is_empty());
    }
}
