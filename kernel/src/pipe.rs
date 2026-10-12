//! パイプ（docs/plans/pipe.md §3.2）
//!
//! 4KB のリングバッファ。読む側（`Reader`）と書く側（`Writer`）は 1 つずつ作り、ファイルの表の
//! 1 件（`fs::Desc`）に入れて `Arc` で共有する。どちらかの最後の参照が無くなると（`Drop`）その側が
//! 閉じたことになる。
//!
//! - 空なら読む側が、満杯なら書く側が待つ（それぞれの `WaitQueue`）
//! - 書く側が閉じていて空なら、読むと 0（終わり）
//! - 読む側が閉じていると、書くと `EPIPE`（ユーザプロセスはシステムコールの出口で EXIT_PIPE で止まる）
//! - 待っている間に止めるよう頼まれたら（Ctrl+C など）`EINTR` で戻る
//!
//! 読める・書けるが変わったら `net::wake_sockets` も呼ぶ（`poll` がそこで待っているため）。

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use vibe_abi::err;

use crate::sync::IrqMutex;
use crate::task::sched;
use crate::task::wait::WaitQueue;

pub const PIPE_SIZE: usize = 4096;

struct State {
    buf: VecDeque<u8>,
    reader_open: bool,
    writer_open: bool,
}

pub struct Pipe {
    state: IrqMutex<State>,
    /// 読む側が待つ（データが来た・書く側が閉じた）
    read_wait: WaitQueue,
    /// 書く側が待つ（空きができた・読む側が閉じた）
    write_wait: WaitQueue,
}

pub struct Reader(Arc<Pipe>);
pub struct Writer(Arc<Pipe>);

pub fn new() -> (Reader, Writer) {
    let pipe = Arc::new(Pipe {
        state: IrqMutex::new(State { buf: VecDeque::with_capacity(PIPE_SIZE), reader_open: true, writer_open: true }),
        read_wait: WaitQueue::new(),
        write_wait: WaitQueue::new(),
    });
    (Reader(pipe.clone()), Writer(pipe))
}

fn interrupted() -> bool {
    crate::process::kill_pending(sched::current_id())
}

impl Reader {
    /// 読んだバイト数（0 は終わり）。空なら何か来るまで待つ
    pub fn read(&self, out: &mut [u8]) -> Result<usize, i32> {
        if out.is_empty() {
            return Ok(0);
        }
        let p = &self.0;
        loop {
            {
                let mut s = p.state.lock();
                if !s.buf.is_empty() {
                    let n = out.len().min(s.buf.len());
                    for (o, b) in out.iter_mut().zip(s.buf.drain(..n)) {
                        *o = b;
                    }
                    drop(s);
                    p.write_wait.notify_all();
                    crate::net::wake_sockets();
                    return Ok(n);
                }
                if !s.writer_open {
                    return Ok(0);
                }
            }
            if interrupted() {
                return Err(err::EINTR);
            }
            p.read_wait.wait_until(|| {
                let s = p.state.lock();
                !s.buf.is_empty() || !s.writer_open || interrupted()
            });
        }
    }

    /// 読んでも待たない（データがあるか、書く側が閉じている）
    pub fn readable(&self) -> bool {
        let s = self.0.state.lock();
        !s.buf.is_empty() || !s.writer_open
    }
}

impl Writer {
    /// 全部書くまで（満杯なら空くのを待って）書く。途中で止めるよう頼まれたら、そこまでのバイト数
    /// （1 バイトも書いていなければ EINTR）。読む側が閉じていれば EPIPE
    pub fn write(&self, data: &[u8]) -> Result<usize, i32> {
        let p = &self.0;
        let mut done = 0;
        while done < data.len() {
            {
                let mut s = p.state.lock();
                if !s.reader_open {
                    return Err(err::EPIPE);
                }
                let n = (PIPE_SIZE - s.buf.len()).min(data.len() - done);
                if n > 0 {
                    s.buf.extend(&data[done..done + n]);
                    done += n;
                    drop(s);
                    p.read_wait.notify_all();
                    crate::net::wake_sockets();
                    continue;
                }
            }
            if interrupted() {
                return if done > 0 { Ok(done) } else { Err(err::EINTR) };
            }
            p.write_wait.wait_until(|| {
                let s = p.state.lock();
                s.buf.len() < PIPE_SIZE || !s.reader_open || interrupted()
            });
        }
        Ok(done)
    }

    /// 書いても待たない（空きがあるか、読む側が閉じている）
    pub fn writable(&self) -> bool {
        let s = self.0.state.lock();
        s.buf.len() < PIPE_SIZE || !s.reader_open
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.state.lock().reader_open = false;
        self.0.write_wait.notify_all();
        crate::net::wake_sockets();
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.0.state.lock().writer_open = false;
        self.0.read_wait.notify_all();
        crate::net::wake_sockets();
    }
}
