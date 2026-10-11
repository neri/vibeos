//! 固定長のリングバッファ（ヒープを使わない）

pub struct Ring<T: Copy, const N: usize> {
    buf: [T; N],
    head: usize,
    len: usize,
}

impl<T: Copy, const N: usize> Ring<T, N> {
    pub const fn new(fill: T) -> Self {
        Ring { buf: [fill; N], head: 0, len: 0 }
    }

    /// 満杯なら捨てて false を返す
    pub fn push(&mut self, v: T) -> bool {
        if self.len == N {
            return false;
        }
        self.buf[(self.head + self.len) % N] = v;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let v = self.buf[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Some(v)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
