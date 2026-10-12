//! プログラムの起動（posix_spawn と同じ「起動するときに引数・環境・入出力を決める」形）
//!
//! ```ignore
//! let pid = Command::new("grep").arg("ELF").stdin(read_fd).spawn()?;
//! let code = sys::wait(pid);
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use crate::{env, sys};

pub struct Command {
    path: String,
    args: Vec<String>,
    /// None なら今の環境をそのまま渡す
    env: Option<Vec<String>>,
    fds: [i32; 3],
}

impl Command {
    /// `path` は名前だけなら /BIN/名前.ELF。`argv[0]` は `path`
    pub fn new(path: &str) -> Command {
        Command { path: String::from(path), args: alloc::vec![String::from(path)], env: None, fds: [-1; 3] }
    }

    pub fn arg(mut self, a: &str) -> Command {
        self.args.push(String::from(a));
        self
    }

    pub fn args<'a>(mut self, list: impl IntoIterator<Item = &'a str>) -> Command {
        self.args.extend(list.into_iter().map(String::from));
        self
    }

    /// `argv[0]` を変える
    pub fn arg0(mut self, name: &str) -> Command {
        self.args[0] = String::from(name);
        self
    }

    fn env_list(&mut self) -> &mut Vec<String> {
        self.env.get_or_insert_with(env::entries)
    }

    /// 子の環境変数を足す・変える
    pub fn env(mut self, name: &str, value: &str) -> Command {
        let entry = alloc::format!("{name}={value}");
        let list = self.env_list();
        match list.iter().position(|e| e.split_once('=').map_or(e.as_str(), |(k, _)| k) == name) {
            Some(i) => list[i] = entry,
            None => list.push(entry),
        }
        self
    }

    /// 子の環境を空にする
    pub fn env_clear(mut self) -> Command {
        self.env = Some(Vec::new());
        self
    }

    pub fn stdin(mut self, fd: i32) -> Command {
        self.fds[0] = fd;
        self
    }

    pub fn stdout(mut self, fd: i32) -> Command {
        self.fds[1] = fd;
        self
    }

    pub fn stderr(mut self, fd: i32) -> Command {
        self.fds[2] = fd;
        self
    }

    /// 起動してタスク ID を返す（エラーは正のエラー番号）
    pub fn spawn(&self) -> Result<i32, i32> {
        let argv = join_nul(self.args.iter().map(String::as_str));
        let envp = match &self.env {
            Some(list) => join_nul(list.iter().map(String::as_str)),
            None => join_nul(env::entries().iter().map(String::as_str)),
        };
        match sys::spawn_ex(&self.path, &argv, &envp, self.fds) {
            pid if pid >= 0 => Ok(pid),
            e => Err(-e),
        }
    }

    /// 起動して終わるのを待ち、終了コードを返す
    pub fn status(&self) -> Result<i32, i32> {
        let pid = self.spawn()?;
        Ok(sys::wait(pid))
    }
}

/// 各要素の後ろに 0 を付けて並べる
fn join_nul<'a>(list: impl Iterator<Item = &'a str>) -> Vec<u8> {
    let mut out = Vec::new();
    for s in list {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    out
}
