//! 1 行の実行
//!
//! - 内蔵コマンドが 1 つだけなら、このプロセスの中で動かす（`cd` などが効くように）。リダイレクトが
//!   あれば、その間だけ fd 0〜2 をつなぎ替える（`dup2` で控えてから戻す）
//! - パイプの中か `&` の内蔵コマンドは、`fork` が無いので子の `sh -c '…'` として起動する
//! - ほかは /BIN のプログラムとして `spawn_ex` で、fd 0〜2 をパイプ・ファイルにして起動する。
//!   シェルが持っているパイプの端は起動した後すぐ閉じる（でないと読む側に終わりが来ない）

use alloc::string::String;
use alloc::vec::Vec;

use vibe_rt::abi::{self, OPEN_APPEND, OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE};
use vibe_rt::process::Command;
use vibe_rt::{eprintln, sys};
use vibe_shell::{Pipeline, Redirect, Target};

use crate::{Shell, builtins};

/// 終了コード: コマンドが見つからない
const NOT_FOUND: i32 = 127;

/// 1 行を実行する（終了コードは sh.status）
pub fn execute(sh: &mut Shell, line: &str) {
    let p = match vibe_shell::parse(line) {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            eprintln!("syntax error: {e}");
            sh.status = 2;
            return;
        }
    };
    let first = &p.commands[0];
    if p.commands.len() == 1 && !p.background {
        if let Some(b) = builtins::find(&first.argv[0]) {
            let argv: Vec<&str> = first.argv.iter().map(String::as_str).collect();
            sh.status = match Saved::redirect(&first.redirects) {
                Ok(saved) => {
                    let code = (b.run)(sh, &argv);
                    saved.restore();
                    code
                }
                Err(msg) => {
                    eprintln!("{msg}");
                    1
                }
            };
            return;
        }
    }
    sh.status = run_pipeline(&p);
}

fn open_flags(t: &Target) -> u32 {
    match t {
        Target::Write(_) => OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE,
        Target::Append(_) => OPEN_WRITE | OPEN_CREATE | OPEN_APPEND,
        _ => 0,
    }
}

/// リダイレクトのファイルを開く
fn open_target(t: &Target) -> Result<i32, String> {
    let (Target::Read(path) | Target::Write(path) | Target::Append(path)) = t else { unreachable!() };
    let fd = sys::open_with(path, open_flags(t));
    if fd < 0 { Err(alloc::format!("{path}: {}", abi::err::name(-fd))) } else { Ok(fd) }
}

/// 控えておく fd（0〜2 を BACKUP + n に写す）
const BACKUP: i32 = 29;

/// 内蔵コマンドのために fd 0〜2 をつなぎ替えたもの。`restore` で元に戻す
struct Saved {
    /// fd n を控えたか（閉じていた fd は控えられない）
    saved: [bool; 3],
    active: bool,
}

impl Saved {
    fn redirect(redirects: &[Redirect]) -> Result<Saved, String> {
        if redirects.is_empty() {
            return Ok(Saved { saved: [false; 3], active: false });
        }
        let mut s = Saved { saved: [false; 3], active: true };
        for fd in 0..3 {
            s.saved[fd as usize] = sys::dup2(fd, BACKUP + fd) >= 0;
        }
        for r in redirects {
            let fd = r.fd as i32;
            match &r.target {
                Target::Dup(m) => {
                    sys::dup2(*m as i32, fd);
                }
                t => match open_target(t) {
                    Ok(f) => {
                        sys::dup2(f, fd);
                        sys::close(f);
                    }
                    Err(msg) => {
                        s.restore();
                        return Err(msg);
                    }
                },
            }
        }
        Ok(s)
    }

    fn restore(self) {
        if !self.active {
            return;
        }
        for fd in 0..3 {
            if self.saved[fd as usize] {
                sys::dup2(BACKUP + fd, fd);
                sys::close(BACKUP + fd);
            } else {
                sys::close(fd);
            }
        }
    }
}

/// `sh -c` に渡せるように引用する（'...' の中の ' は '\'' にする）
fn quote(argv: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push('\'');
        out.push_str(&a.replace('\'', "'\\''"));
        out.push('\'');
    }
    out
}

/// コマンドをパイプでつないで起動し、全部が終わるのを待つ（`&` なら待たない）。最後のコマンドの終了コード
fn run_pipeline(p: &Pipeline) -> i32 {
    let n = p.commands.len();
    // (タスク ID, 名前, 最後のコマンドか)
    let mut started: Vec<(i32, &str, bool)> = Vec::new();
    let mut status = 0;
    let mut prev_read: Option<i32> = None;
    for (i, cmd) in p.commands.iter().enumerate() {
        let last = i + 1 == n;
        let (next_read, write) = if last {
            (None, None)
        } else {
            match sys::pipe() {
                Ok((r, w)) => (Some(r), Some(w)),
                Err(e) => {
                    eprintln!("sh: pipe: {}", abi::err::name(e));
                    break;
                }
            }
        };
        // 子の fd 0〜2（-1 はシェルの同じ番号）
        let mut fds = [prev_read.unwrap_or(-1), write.unwrap_or(-1), -1];
        let mut opened = Vec::new();
        let mut ok = true;
        for r in &cmd.redirects {
            let fd = r.fd as usize;
            match &r.target {
                Target::Dup(m) => fds[fd] = if fds[*m as usize] < 0 { *m as i32 } else { fds[*m as usize] },
                t => match open_target(t) {
                    Ok(f) => {
                        opened.push(f);
                        fds[fd] = f;
                    }
                    Err(msg) => {
                        eprintln!("{msg}");
                        ok = false;
                        break;
                    }
                },
            }
        }
        let name = cmd.argv[0].as_str();
        if ok {
            // argv[0] は打った名前のまま渡す
            let command = if builtins::find(name).is_some() {
                Command::new("sh").arg("-c").arg(&quote(&cmd.argv))
            } else {
                Command::new(name).args(cmd.argv[1..].iter().map(String::as_str))
            };
            match command.stdin(fds[0]).stdout(fds[1]).stderr(fds[2]).spawn() {
                Ok(pid) => started.push((pid, name, last)),
                Err(abi::err::ENOENT) => {
                    eprintln!("unknown command: {name}");
                    status = NOT_FOUND;
                }
                Err(e) => {
                    eprintln!("{name}: {}", abi::err::name(e));
                    status = NOT_FOUND;
                }
            }
        }
        // シェルが持っている分を閉じる
        for f in opened.into_iter().chain(write).chain(prev_read) {
            sys::close(f);
        }
        prev_read = next_read;
    }
    if let Some(r) = prev_read {
        sys::close(r);
    }
    if p.background {
        for (pid, name, _) in started {
            sys::detach(pid);
            eprintln!("[{pid}] {name}");
        }
        return 0;
    }
    // 前面のプロセスにする（Ctrl+C で全部を止められる）
    let pids: Vec<i32> = started.iter().map(|s| s.0).collect();
    sys::set_foreground(&pids);
    for &(pid, name, last) in &started {
        let code = sys::wait(pid);
        if last {
            status = code;
            if code != 0 {
                eprintln!("[{name} exited with {code}]");
            }
        }
    }
    sys::set_foreground(&[]);
    status
}
