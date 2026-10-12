//! スクリプトの言語のテスト（偽物の Host で）

use std::collections::BTreeMap;

use crate::ParseError;
use crate::ast::{Kind, parse_script, words_of};
use crate::interp::{Host, Shell, glob_match, is_incomplete};

/// 起動したプログラム
#[derive(Debug, Clone)]
struct Spawned {
    argv: Vec<String>,
    env: Vec<String>,
    fds: [i32; 3],
}

/// 偽物: 標準出力・標準エラーは文字列、ファイルは表、パイプはバッファ。プログラムは
/// `echoargs`（引数を標準出力へ）・`exit N`・`sh -c`（入れ子の Shell で動かす）・ほかは codes の終了コード
#[derive(Default)]
struct Fake {
    stdout: String,
    stderr: String,
    files: BTreeMap<String, String>,
    dirs: Vec<String>,
    /// fd → 書く先（"file:PATH" / "pipe:N"）
    fds: BTreeMap<i32, String>,
    pipes: Vec<Vec<u8>>,
    next_fd: i32,
    /// 今の fd 0〜2（-1 は元の標準入出力）
    cur: [i32; 3],
    saved: Vec<[i32; 3]>,
    spawns: Vec<Spawned>,
    codes: BTreeMap<String, i32>,
    exited: BTreeMap<i32, i32>,
    next_pid: i32,
    foreground: Vec<Vec<i32>>,
    detached: Vec<i32>,
    /// interrupted() がこの回数呼ばれたら true を返す
    interrupt_at: Option<u32>,
    interrupt_calls: u32,
    cwd_names: Vec<String>,
}

impl Fake {
    fn new() -> Fake {
        Fake { next_fd: 10, next_pid: 100, cur: [-1; 3], ..Default::default() }
    }

    fn alloc_fd(&mut self, target: String) -> i32 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(fd, target);
        fd
    }

    /// fd（-1 なら元の番号 n）に書く
    fn write_to(&mut self, fd: i32, n: usize, data: &str) {
        let fd = if fd < 0 { self.cur[n] } else { fd };
        if fd < 0 {
            if n == 2 { self.stderr.push_str(data) } else { self.stdout.push_str(data) }
            return;
        }
        if (0..3).contains(&fd) {
            return self.write_to(self.cur[fd as usize], fd as usize, data);
        }
        match self.fds.get(&fd).cloned() {
            Some(t) if t.starts_with("file:") => self.files.entry(t[5..].to_string()).or_default().push_str(data),
            Some(t) if t.starts_with("pipew:") => {
                let i: usize = t[6..].parse().unwrap();
                self.pipes[i].extend_from_slice(data.as_bytes());
            }
            other => panic!("write to fd {fd} ({other:?})"),
        }
    }
}

impl Host for Fake {
    fn spawn(&mut self, argv: &[String], env: &[String], fds: [i32; 3]) -> Result<i32, i32> {
        self.spawns.push(Spawned { argv: argv.to_vec(), env: env.to_vec(), fds });
        let code = match argv[0].as_str() {
            "echoargs" => {
                let text = format!("{}\n", argv[1..].join(" "));
                self.write_to(fds[1], 1, &text);
                0
            }
            "exit" => argv[1].parse().unwrap(),
            "sh" if argv.get(1).map(String::as_str) == Some("-c") => {
                let mut inner = Fake::new();
                inner.files = self.files.clone();
                let mut sh = Shell::new(inner);
                sh.arg0 = argv.get(3).cloned().unwrap_or_default();
                sh.args = argv.iter().skip(4).cloned().collect();
                for e in env {
                    let (k, v) = e.split_once('=').unwrap();
                    sh.set_var(k, v);
                }
                sh.run(&argv[2]);
                let out = sh.host.stdout.clone();
                self.files = sh.host.files.clone();
                self.write_to(fds[1], 1, &out);
                sh.exit.unwrap_or(sh.status)
            }
            name => match self.codes.get(name) {
                Some(&c) => c,
                None => return Err(vibe_abi::err::ENOENT),
            },
        };
        let pid = self.next_pid;
        self.next_pid += 1;
        self.exited.insert(pid, code);
        Ok(pid)
    }
    fn wait(&mut self, pid: i32) -> i32 {
        self.exited.remove(&pid).expect("wait for an unknown pid")
    }
    fn detach(&mut self, pid: i32) {
        self.detached.push(pid);
    }
    fn foreground(&mut self, pids: &[i32]) {
        self.foreground.push(pids.to_vec());
    }
    fn pipe(&mut self) -> Result<(i32, i32), i32> {
        self.pipes.push(Vec::new());
        let i = self.pipes.len() - 1;
        let r = self.alloc_fd(format!("piper:{i}"));
        let w = self.alloc_fd(format!("pipew:{i}"));
        Ok((r, w))
    }
    fn open(&mut self, path: &str, flags: u32) -> Result<i32, i32> {
        use vibe_abi::{OPEN_APPEND, OPEN_WRITE};
        if flags & OPEN_WRITE == 0 {
            if !self.files.contains_key(path) {
                return Err(vibe_abi::err::ENOENT);
            }
            return Ok(self.alloc_fd(format!("read:{path}")));
        }
        if flags & OPEN_APPEND == 0 {
            self.files.insert(path.to_string(), String::new());
        }
        Ok(self.alloc_fd(format!("file:{path}")))
    }
    fn read(&mut self, fd: i32, buf: &mut [u8]) -> i32 {
        let t = self.fds[&fd].clone();
        let i: usize = t.strip_prefix("piper:").unwrap().parse().unwrap();
        let n = buf.len().min(self.pipes[i].len());
        buf[..n].copy_from_slice(&self.pipes[i][..n]);
        self.pipes[i].drain(..n);
        n as i32
    }
    fn close(&mut self, fd: i32) {
        assert!(self.fds.remove(&fd).is_some(), "close of unknown fd {fd}");
    }
    fn redirect(&mut self, fds: [i32; 3]) -> [i32; 3] {
        self.saved.push(self.cur);
        for i in 0..3 {
            if fds[i] >= 0 {
                self.cur[i] = if fds[i] < 3 { self.cur[fds[i] as usize] } else { fds[i] };
            }
        }
        [0; 3]
    }
    fn restore(&mut self, _: [i32; 3]) {
        self.cur = self.saved.pop().unwrap();
    }
    fn write(&mut self, fd: i32, data: &[u8]) {
        let s = String::from_utf8_lossy(data).into_owned();
        self.write_to(fd, fd as usize, &s);
    }
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, i32> {
        self.files.get(path).map(|s| s.as_bytes().to_vec()).ok_or(vibe_abi::err::ENOENT)
    }
    fn stat(&mut self, path: &str) -> Option<bool> {
        if self.dirs.iter().any(|d| d == path) {
            Some(true)
        } else {
            self.files.contains_key(path).then_some(false)
        }
    }
    fn list_dir(&mut self) -> Vec<String> {
        self.cwd_names.clone()
    }
    fn interrupted(&mut self) -> bool {
        self.interrupt_calls += 1;
        self.interrupt_at.is_some_and(|n| self.interrupt_calls >= n)
    }
    fn is_builtin(&self, name: &str) -> bool {
        name == "appcmd"
    }
    fn builtin(&mut self, argv: &[String]) -> i32 {
        let text = format!("app:{}\n", argv[1..].join(","));
        self.write_to(-1, 1, &text);
        argv.len() as i32 - 1
    }
}

fn run(src: &str) -> Shell<Fake> {
    let mut sh = Shell::new(Fake::new());
    sh.run(src);
    assert!(sh.host.fds.is_empty(), "fds left open: {:?}", sh.host.fds);
    sh
}

fn out(src: &str) -> String {
    run(src).host.stdout
}

#[test]
fn parse_structure() {
    let l = parse_script("a 1; b && c || d\nif x; then y; elif z; then w; else v; fi > F &").unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!(words_of(&l[0].and_or.first.commands[0]), ["a", "1"]);
    assert_eq!(l[1].and_or.rest.len(), 2);
    assert!(l[2].background);
    let c = &l[2].and_or.first.commands[0];
    let Kind::If { arms, otherwise } = &c.kind else { panic!() };
    assert_eq!(arms.len(), 2);
    assert!(otherwise.is_some());
    assert_eq!(c.redirects.len(), 1);
    // 複合コマンドの元の文字列の範囲
    let src = "x | for i in a b; do echo $i; done | y";
    let l = parse_script(src).unwrap();
    let c = &l[0].and_or.first.commands[1];
    assert_eq!(&src[c.span.0..c.span.1], "for i in a b; do echo $i; done");
    // 空の中身・コメント・行の続き
    assert!(parse_script("while true; do; done").is_ok());
    let l = parse_script("echo a # comment\necho \\\nb").unwrap();
    assert_eq!(words_of(&l[1].and_or.first.commands[0]), ["echo", "b"]);
}

#[test]
fn parse_errors_and_incomplete() {
    for src in ["if true; then", "while x", "for i in 1 2", "echo 'a", "echo \"a", "a &&", "a |", "if a; then b; else", "echo $(ls"] {
        assert!(is_incomplete(src), "{src}");
    }
    assert!(!is_incomplete("echo done"));
    assert_eq!(parse_script("fi"), Err(ParseError::Unexpected("fi".into())));
    assert_eq!(parse_script("echo a; then"), Err(ParseError::Unexpected("then".into())));
    assert_eq!(parse_script("a && && b"), Err(ParseError::Unexpected("&&".into())));
    assert_eq!(parse_script("(a)"), Err(ParseError::Unsupported('(')));
    assert_eq!(parse_script("echo ${1x}"), Err(ParseError::BadSubstitution));
    assert_eq!(parse_script("ls >"), Err(ParseError::MissingTarget(">")));
    assert_eq!(parse_script("for 1 in a; do x; done"), Err(ParseError::Unexpected("for: bad variable name".into())));
    // 予約語は先頭だけ
    assert_eq!(out("echo if then fi"), "if then fi\n");
}

#[test]
fn variables_and_quotes() {
    assert_eq!(out("X=hello; echo $X ${X}! \"$X world\" '$X'"), "hello hello! hello world $X\n");
    assert_eq!(out("X='a  b'; echo [$X] \"[$X]\""), "[a b] [a  b]\n");
    assert_eq!(out("echo \"\" x '' y | cat"), "");
    assert_eq!(out("false; echo $?; true; echo $?"), "1\n0\n");
    assert_eq!(out("echo $NOPE.$UNSET."), "..\n");
    assert_eq!(out("echo \\$X \"\\$X\" a\\ b"), "$X $X a b\n");
    let mut sh = Shell::new(Fake::new());
    sh.arg0 = "s.sh".into();
    sh.args = vec!["one".into(), "two words".into()];
    sh.run("echo $0 $# $1 \"$2\"; for a in \"$@\"; do echo [$a]; done; for a in $@; do echo -$a-; done");
    assert_eq!(sh.host.stdout, "s.sh 2 one two words\n[one]\n[two words]\n-one-\n-two-\n-words-\n");
    sh.host.stdout.clear();
    sh.run("shift; echo $1 $#");
    assert_eq!(sh.host.stdout, "two words 1\n");
}

#[test]
fn arithmetic() {
    assert_eq!(out("echo $((1 + 2 * 3)) $(( (1+2)*3 )) $((7 / 2)) $((7 % 3)) $((-4 + 1))"), "7 9 3 1 -3\n");
    assert_eq!(out("I=5; echo $((I * 2)) $(($I + 1)) $((I > 3)) $((I == 4)) $((I != 4 && 1))"), "10 6 1 0 1\n");
    let sh = run("echo $((1 / 0))");
    assert_eq!(sh.host.stdout, "0\n");
    assert!(sh.host.stderr.contains("division by zero"));
    assert_eq!(crate::arith::eval("2 <= 2", |_| 0), Ok(1));
    assert_eq!(crate::arith::eval("!0 + !5", |_| 0), Ok(1));
    assert!(crate::arith::eval("1 +", |_| 0).is_err());
}

#[test]
fn control_flow() {
    let sum = "S=0; I=1\nwhile [ $I -le 10 ]; do\n  S=$((S + I))\n  I=$((I + 1))\ndone\necho sum=$S";
    assert_eq!(out(sum), "sum=55\n");
    let fizz = "for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do\n\
                if [ $((i % 15)) -eq 0 ]; then echo FizzBuzz\n\
                elif [ $((i % 3)) -eq 0 ]; then echo Fizz\n\
                elif [ $((i % 5)) -eq 0 ]; then echo Buzz\n\
                else echo $i; fi\ndone";
    let o = out(fizz);
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.len(), 15);
    assert_eq!(lines[2], "Fizz");
    assert_eq!(lines[4], "Buzz");
    assert_eq!(lines[14], "FizzBuzz");
    assert_eq!(out("for i in a b c d; do if [ $i = b ]; then continue; fi; if [ $i = d ]; then break; fi; echo $i; done"), "a\nc\n");
    assert_eq!(out("for i in 1 2; do for j in x y; do if [ $j = y ]; then continue 2; fi; echo $i$j; done; done"), "1x\n2x\n");
    assert_eq!(out("true && echo a || echo b; false && echo c || echo d; false || false || echo e"), "a\nd\ne\n");
    assert_eq!(out("if false; then echo no; fi; echo $?"), "0\n");
    let sh = run("echo a; exit 3; echo b");
    assert_eq!((sh.host.stdout.as_str(), sh.exit), ("a\n", Some(3)));
    assert_eq!(run("while true; do exit 4; done; echo no").exit, Some(4));
    assert!(run("break").host.stderr.contains("only in a loop"));
}

#[test]
fn test_builtin() {
    let mut sh = Shell::new(Fake::new());
    sh.host.files.insert("A.TXT".into(), "x".into());
    sh.host.dirs.push("DOCS".into());
    for (expr, want) in [
        ("-e A.TXT", 0), ("-f A.TXT", 0), ("-d A.TXT", 1), ("-d DOCS", 0), ("-e NO", 1), ("! -e NO", 0),
        ("-z ''", 0), ("-n ''", 1), ("abc", 0), ("''", 1), ("a = a", 0), ("a != a", 1), ("3 -lt 10", 0),
        ("10 -lt 3", 1), ("2 -ge 2", 0), ("1 -eq x", 2), ("", 1),
    ] {
        sh.run(&format!("test {expr}"));
        assert_eq!(sh.status, want, "test {expr}");
        sh.run(&format!("[ {expr} ]"));
        assert_eq!(sh.status, want, "[ {expr} ]");
    }
    sh.run("[ 1 = 1");
    assert_eq!(sh.status, 2);
}

#[test]
fn programs_env_and_status() {
    let mut sh = Shell::new(Fake::new());
    sh.host.codes.insert("prog".into(), 3);
    sh.interactive = true;
    sh.run("X=1; export Y=2; export X; Z=3; prog a 'b c'");
    let s = sh.host.spawns.last().unwrap().clone();
    assert_eq!(s.argv, ["prog", "a", "b c"]);
    assert_eq!(s.env, ["X=1", "Y=2"]);
    assert_eq!(s.fds, [-1, -1, -1]);
    assert_eq!(sh.status, 3);
    assert_eq!(sh.host.stderr, "[prog exited with 3]\n");
    assert_eq!(sh.host.foreground, [vec![100], vec![]]);
    // その時だけの環境。条件の中では失敗を知らせない
    sh.host.stderr.clear();
    sh.run("W=9 prog; if prog; then :; fi; prog || true");
    assert_eq!(sh.host.spawns[1].env, ["W=9", "X=1", "Y=2"]);
    assert_eq!(sh.host.stderr, "[prog exited with 3]\n");
    sh.run("nosuch x");
    assert_eq!(sh.status, 127);
    assert!(sh.host.stderr.ends_with("unknown command: nosuch\n"));
    sh.run("unset X; env");
    assert_eq!(sh.host.stdout, "Y=2\n");
    sh.host.stdout.clear();
    sh.run("set");
    assert_eq!(sh.host.stdout, "Y=2\nZ=3\n");
}

#[test]
fn pipelines_and_subshells() {
    let mut sh = Shell::new(Fake::new());
    sh.host.codes.insert("grep".into(), 1);
    sh.run("echoargs hi | grep h | wc");
    let sp = &sh.host.spawns;
    assert_eq!(sp.len(), 3);
    assert_eq!(sp[2].argv, ["wc"], "wc is unknown");
    // 1 本目のパイプの書く側・読む側
    assert_eq!(sp[0].fds, [-1, 11, -1]);
    assert_eq!(sp[1].fds[0], 10);
    assert_eq!(sh.status, 127);
    assert!(sh.host.fds.is_empty());
    // 内蔵コマンドと複合コマンドはパイプの中では子の sh（展開した引数を引用し直す・元の文字列）
    let mut sh = Shell::new(Fake::new());
    sh.host.codes.insert("cat".into(), 0);
    sh.run("X=\"it's\"; echo $X | cat; for i in 1 2; do echo $i; done | cat");
    assert_eq!(sh.host.spawns[0].argv, ["sh", "-c", "'echo' 'it'\\''s'", "sh"]);
    assert_eq!(sh.host.spawns[2].argv[2], "for i in 1 2; do echo $i; done");
    // export していない変数も子の sh には渡す
    assert_eq!(sh.host.spawns[2].env, ["X=it's"]);
    // 裏で動かす
    let mut sh = Shell::new(Fake::new());
    sh.host.codes.insert("srv".into(), 0);
    sh.interactive = true;
    sh.run("srv 80 & echo next");
    assert_eq!(sh.host.detached, [100]);
    assert_eq!(sh.host.stderr, "[100] srv\n");
    assert_eq!(sh.host.stdout, "next\n");
}

#[test]
fn redirects() {
    let sh = run("echo one > A.TXT; echo two >> A.TXT; echoargs three >> A.TXT; appcmd x y > B.TXT");
    assert_eq!(sh.host.files["A.TXT"], "one\ntwo\nthree\n");
    assert_eq!(sh.host.files["B.TXT"], "app:x,y\n");
    assert_eq!(sh.host.stdout, "");
    let sh = run("for i in 1 2; do echo $i; done > C.TXT; echo e 2>&1 > D.TXT");
    assert_eq!(sh.host.files["C.TXT"], "1\n2\n");
    assert_eq!(sh.host.files["D.TXT"], "e\n");
    let sh = run("cat < NOPE.TXT; echo $?");
    assert_eq!(sh.host.stdout, "1\n");
    assert!(sh.host.stderr.contains("NOPE.TXT: not found"));
    let sh = run("F=X.TXT; echo v > $F; > EMPTY.TXT");
    assert_eq!(sh.host.files["X.TXT"], "v\n");
    assert_eq!(sh.host.files["EMPTY.TXT"], "");
}

#[test]
fn globbing() {
    assert!(glob_match("*.txt", "HELLO.TXT"));
    assert!(glob_match("H?LLO.*", "hello.txt"));
    assert!(!glob_match("*.TXT", "HELLO.BMP"));
    assert!(glob_match("a\\*", "A*"));
    assert!(!glob_match("a\\*", "AB"));
    let mut sh = Shell::new(Fake::new());
    sh.host.cwd_names = vec!["HELLO.TXT".into(), "B.TXT".into(), "PIC.BMP".into(), "..".into()];
    sh.run("echo *.TXT; echo '*.TXT' \"*\".TXT; echo *.NONE; echo *");
    assert_eq!(sh.host.stdout, "B.TXT HELLO.TXT\n*.TXT *.TXT\n*.NONE\nB.TXT HELLO.TXT PIC.BMP\n");
}

#[test]
fn command_substitution() {
    assert_eq!(out("X=$(echo hi there); echo [$X] [$(echo a b)]"), "[hi there] [a b]\n");
    assert_eq!(out("echo $(( $(echo 6) * 7 ))"), "42\n");
}

#[test]
fn interrupt_stops_loops() {
    let mut sh = Shell::new(Fake::new());
    sh.host.interrupt_at = Some(50);
    sh.run("while true; do; done; echo after");
    assert_eq!(sh.status, 130);
    assert_eq!(sh.host.stdout, "");
    let mut sh = Shell::new(Fake::new());
    sh.host.interrupt_at = Some(3);
    sh.run("for i in 1 2 3 4 5 6; do echo $i; done");
    assert!(sh.host.stdout.lines().count() < 6);
}

#[test]
fn dot_runs_in_this_shell() {
    let mut sh = Shell::new(Fake::new());
    sh.host.files.insert("LIB.SH".into(), "V=from_lib\necho $1".into());
    sh.run(". LIB.SH arg; echo $V $#");
    assert_eq!(sh.host.stdout, "arg\nfrom_lib 0\n");
}
