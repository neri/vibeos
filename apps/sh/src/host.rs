//! `vibe_shell::interp::Host` をシステムコールでつなぐ

use alloc::string::String;
use alloc::vec::Vec;

use vibe_line::LineEditor;
use vibe_rt::abi::{self, DirEntry, Stat};
use vibe_rt::{io, sys};
use vibe_shell::interp::Host;

use crate::builtins;

/// fd 0〜2 をつなぎ替える間、元の fd を控えておく番号（BACKUP + n）
const BACKUP: i32 = 29;

pub struct AppHost {
    pub editor: LineEditor,
    /// 対話のシェル: 前面のプロセスを決め、Ctrl+C を自分で受ける
    pub interactive: bool,
    me: i32,
}

impl AppHost {
    pub fn new(interactive: bool) -> AppHost {
        AppHost { editor: LineEditor::new(), interactive, me: sys::getpid() }
    }

    /// 対話のシェルで、コマンドを実行している間は自分を前面に（Ctrl+C を受けて止めるため）
    pub fn set_running(&mut self, running: bool) {
        if self.interactive {
            let me = [self.me];
            sys::set_foreground(if running { &me[..] } else { &[] });
        }
    }
}

/// カレントディレクトリ
pub fn cwd() -> String {
    sys::getcwd().unwrap_or_else(|_| String::from("?"))
}

/// 各要素の後ろに 0 を付けて並べる
fn join_nul(list: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for s in list {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    out
}

impl Host for AppHost {
    fn spawn(&mut self, argv: &[String], env: &[String], fds: [i32; 3]) -> Result<i32, i32> {
        match sys::spawn_ex(&argv[0], &join_nul(argv), &join_nul(env), fds) {
            pid if pid >= 0 => Ok(pid),
            e => Err(-e),
        }
    }

    fn wait(&mut self, pid: i32) -> i32 {
        sys::wait(pid)
    }

    fn detach(&mut self, pid: i32) {
        sys::detach(pid);
    }

    fn foreground(&mut self, pids: &[i32]) {
        if self.interactive {
            let mut all = alloc::vec![self.me];
            all.extend_from_slice(pids);
            sys::set_foreground(&all);
        }
    }

    fn pipe(&mut self) -> Result<(i32, i32), i32> {
        sys::pipe()
    }

    fn open(&mut self, path: &str, flags: u32) -> Result<i32, i32> {
        match sys::open_with(path, flags) {
            fd if fd >= 0 => Ok(fd),
            e => Err(-e),
        }
    }

    fn read(&mut self, fd: i32, buf: &mut [u8]) -> i32 {
        sys::read(fd, buf)
    }

    fn close(&mut self, fd: i32) {
        sys::close(fd);
    }

    /// 戻すための値: 1 = BACKUP + n に控えた、0 = 閉じていた（戻すときに閉じる）、-1 = 変えていない
    fn redirect(&mut self, fds: [i32; 3]) -> [i32; 3] {
        let mut saved = [-1; 3];
        for i in 0..3 {
            let f = fds[i];
            if f < 0 {
                continue;
            }
            saved[i] = (sys::dup2(i as i32, BACKUP + i as i32) >= 0) as i32;
            // 先につなぎ替えた 0〜2 を指すなら、元のもの（控え）を使う
            let src = if (0..i as i32).contains(&f) && saved[f as usize] == 1 { BACKUP + f } else { f };
            sys::dup2(src, i as i32);
        }
        saved
    }

    fn restore(&mut self, saved: [i32; 3]) {
        for (i, &s) in saved.iter().enumerate() {
            let i = i as i32;
            match s {
                1 => {
                    sys::dup2(BACKUP + i, i);
                    sys::close(BACKUP + i);
                }
                0 => {
                    sys::close(i);
                }
                _ => {}
            }
        }
    }

    fn write(&mut self, fd: i32, data: &[u8]) {
        let _ = io::write_all(fd, data);
    }

    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, i32> {
        vibe_rt::read_file(path)
    }

    fn stat(&mut self, path: &str) -> Option<bool> {
        let mut st = Stat::default();
        (sys::stat(path, &mut st) >= 0).then_some(st.is_dir != 0)
    }

    fn list_dir(&mut self) -> Vec<String> {
        let mut v = alloc::vec![DirEntry::EMPTY; 256];
        let n = sys::readdir(".", &mut v).max(0) as usize;
        v[..n].iter().map(|e| String::from(e.name())).collect()
    }

    fn interrupted(&mut self) -> bool {
        self.interactive && vibe_rt::net::ctrl_c_pressed()
    }

    fn is_builtin(&self, name: &str) -> bool {
        builtins::find(name).is_some()
    }

    fn builtin(&mut self, argv: &[String]) -> i32 {
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        match builtins::find(&argv[0]) {
            Some(b) => (b.run)(self, &args),
            None => abi::err::ENOENT,
        }
    }
}
