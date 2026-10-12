//! スクリプトの実行（docs/plans/user-shell.md §3.2・§3.3）
//!
//! プロセスの起動・待ち・ファイル・fd のつなぎ替えは「実行する側」（`Host`）に任せる。`sh` は
//! システムコールで、ホストのテストは偽物で実装する。
//!
//! - 内蔵コマンド（言語のもの: `echo`・`test` など。アプリのもの: `Host::builtin`）と `if`・`while`・`for` は
//!   このプロセスの中で動かす。リダイレクトはその間だけ fd 0〜2 をつなぎ替える（`Host::redirect`）
//! - パイプの中・`&` の内蔵コマンドと複合コマンドは、`fork` が無いので子の `sh -c` で動かす。単純な
//!   コマンドは展開した後の引数を引用し直して渡し、複合コマンドは元の文字列と位置引数を渡す（シェルの
//!   変数は、export していないものも環境として渡す）
//! - 変数の値は、引用していないところでは空白で分け、`*` と `?` があればカレントディレクトリの名前で
//!   展開する（合うものが無ければそのまま）

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use vibe_abi::{OPEN_APPEND, OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE, err};

use crate::ParseError;
use crate::arith;
use crate::ast::{AndOr, Command, Connector, Item, Kind, List, Pipeline, Redir, is_name, parse_script};
use crate::lex::{Part, RedirOp, Word};

/// 実行する側
pub trait Host {
    /// プログラムを起動する（`argv[0]` が名前だけなら /BIN）。`fds` は子の fd 0〜2 にする fd（-1 は同じ番号）
    fn spawn(&mut self, argv: &[String], env: &[String], fds: [i32; 3]) -> Result<i32, i32>;
    /// 終わるのを待って終了コード
    fn wait(&mut self, pid: i32) -> i32;
    /// 待たない（裏で動かす）
    fn detach(&mut self, pid: i32);
    /// 前面で動いている子（Ctrl+C で止める相手）。空なら無し
    fn foreground(&mut self, pids: &[i32]);
    fn pipe(&mut self) -> Result<(i32, i32), i32>;
    fn open(&mut self, path: &str, flags: u32) -> Result<i32, i32>;
    fn read(&mut self, fd: i32, buf: &mut [u8]) -> i32;
    fn close(&mut self, fd: i32);
    /// fd 0〜2 をしばらく `fds`（-1 はそのまま）にする。戻すための値を返す
    fn redirect(&mut self, fds: [i32; 3]) -> [i32; 3];
    fn restore(&mut self, saved: [i32; 3]);
    fn write(&mut self, fd: i32, data: &[u8]);
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, i32>;
    /// Some(true) はディレクトリ、Some(false) はファイル、None は無い
    fn stat(&mut self, path: &str) -> Option<bool>;
    /// カレントディレクトリの中の名前（ファイル名の展開）
    fn list_dir(&mut self) -> Vec<String>;
    /// Ctrl+C が押されたか（印は消す）
    fn interrupted(&mut self) -> bool;
    /// アプリの内蔵コマンド（`cd`・`ls` など）
    fn is_builtin(&self, name: &str) -> bool;
    fn builtin(&mut self, argv: &[String]) -> i32;
}

/// 言語の内蔵コマンド（名前, 使い方, 説明）。`help` で出す
pub const BUILTINS: &[(&str, &str, &str)] = &[
    ("echo", "echo [text...]", "print text (-n: no newline at the end)"),
    ("test", "test expr | [ expr ]", "check files, strings and numbers (-e -f -d -z -n = != -eq -lt ...)"),
    ("[", "[ expr ]", "same as test"),
    ("true", "true", "do nothing, successfully"),
    ("false", "false", "do nothing, unsuccessfully"),
    ("export", "export [NAME[=value]...]", "pass variables to programs (no names: list them)"),
    ("unset", "unset NAME...", "remove variables"),
    ("set", "set", "list shell variables"),
    ("env", "env", "list exported variables"),
    ("shift", "shift [n]", "drop the first script arguments"),
    (".", ". file [args...]", "run a script in this shell"),
    ("exit", "exit [code]", "end the shell or script"),
    ("break", "break [n]", "leave a loop"),
    ("continue", "continue [n]", "go to the next round of a loop"),
];

fn is_lang_builtin(name: &str) -> bool {
    BUILTINS.iter().any(|b| b.0 == name) || name == ":"
}

/// コマンドが見つからない
pub const NOT_FOUND: i32 = 127;
/// Ctrl+C で止めた
pub const INTERRUPTED: i32 = 130;

#[derive(Debug, Clone)]
struct Var {
    value: String,
    exported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Next,
    Break(u32),
    Continue(u32),
    /// exit か Ctrl+C: 残りを実行しない
    Stop,
}

pub struct Shell<H: Host> {
    pub host: H,
    vars: BTreeMap<String, Var>,
    /// `$0`
    pub arg0: String,
    /// `$1`...
    pub args: Vec<String>,
    /// `$?`
    pub status: i32,
    /// exit が呼ばれた（終了コード）
    pub exit: Option<i32>,
    /// 対話のとき: 0 以外で終わったプログラムを "[名前 exited with n]" と出し、`&` の番号を出す
    pub interactive: bool,
    /// if・while の条件・`&&`/`||` の左側を実行している（失敗を知らせない）
    cond_depth: u32,
    loop_depth: u32,
    /// 今実行しているスクリプトの文字列（複合コマンドを `sh -c` に渡すとき、位置で切り出す）
    src: String,
}

impl<H: Host> Shell<H> {
    pub fn new(host: H) -> Shell<H> {
        Shell {
            host,
            vars: BTreeMap::new(),
            arg0: String::from("sh"),
            args: Vec::new(),
            status: 0,
            exit: None,
            interactive: false,
            cond_depth: 0,
            loop_depth: 0,
            src: String::new(),
        }
    }

    // --- 変数 -------------------------------------------------------------------

    pub fn var(&self, name: &str) -> Option<String> {
        match name {
            "?" => Some(self.status.to_string()),
            "#" => Some(self.args.len().to_string()),
            "@" => Some(self.args.join(" ")),
            "0" => Some(self.arg0.clone()),
            n if n.bytes().all(|b| b.is_ascii_digit()) => {
                let i: usize = n.parse().ok()?;
                self.args.get(i.checked_sub(1)?).cloned()
            }
            n => self.vars.get(n).map(|v| v.value.clone()),
        }
    }

    pub fn set_var(&mut self, name: &str, value: &str) {
        let exported = self.vars.get(name).is_some_and(|v| v.exported);
        self.vars.insert(String::from(name), Var { value: String::from(value), exported });
    }

    /// 環境変数として持つ（起動するプログラムに渡る）
    pub fn export(&mut self, name: &str, value: Option<&str>) {
        let v = self.vars.entry(String::from(name)).or_insert(Var { value: String::new(), exported: true });
        v.exported = true;
        if let Some(value) = value {
            v.value = String::from(value);
        }
    }

    /// 起動するプログラムに渡す環境（"名前=値"。名前の順）。`all` なら export していないものも。
    /// `extra` はそのコマンドにだけ渡すもの（同じ名前なら上書き）
    fn environment(&self, all: bool, extra: &[(String, String)]) -> Vec<String> {
        let mut env: Vec<String> = Vec::new();
        for (k, v) in self.vars.iter().filter(|(_, v)| all || v.exported) {
            if !extra.iter().any(|e| e.0 == *k) {
                env.push(format!("{k}={}", v.value));
            }
        }
        for (k, v) in extra {
            let entry = format!("{k}={v}");
            let at = env.iter().position(|e| e.as_str() > entry.as_str()).unwrap_or(env.len());
            env.insert(at, entry);
        }
        env
    }

    // --- 実行 -------------------------------------------------------------------

    fn err(&mut self, msg: &str) {
        self.host.write(2, msg.as_bytes());
        self.host.write(2, b"\n");
    }

    /// スクリプト（何行でも）を実行する。文法の誤りは標準エラーに出して終了コード 2
    pub fn run(&mut self, src: &str) {
        let list = match parse_script(src) {
            Ok(l) => l,
            Err(e) => {
                self.err(&format!("syntax error: {e}"));
                self.status = 2;
                return;
            }
        };
        let saved = core::mem::replace(&mut self.src, String::from(src));
        self.exec_list(&list);
        self.src = saved;
    }

    /// Ctrl+C が押されたか（押されたら残りを実行しない）
    fn check_interrupt(&mut self) -> bool {
        if self.host.interrupted() {
            self.status = INTERRUPTED;
            return true;
        }
        false
    }

    fn exec_list(&mut self, list: &List) -> Flow {
        for item in list {
            let flow = if item.background { self.background(item) } else { self.exec_and_or(&item.and_or) };
            if flow != Flow::Next {
                return flow;
            }
            if self.exit.is_some() || self.check_interrupt() {
                return Flow::Stop;
            }
        }
        Flow::Next
    }

    fn exec_and_or(&mut self, ao: &AndOr) -> Flow {
        let n = ao.rest.len();
        // 左側（続きがあるもの）の失敗は知らせない
        self.cond_depth += (n > 0) as u32;
        let mut flow = self.exec_pipeline(&ao.first);
        self.cond_depth -= (n > 0) as u32;
        for (i, (c, p)) in ao.rest.iter().enumerate() {
            if flow != Flow::Next {
                return flow;
            }
            let run = match c {
                Connector::And => self.status == 0,
                Connector::Or => self.status != 0,
            };
            if run {
                let last = i + 1 == n;
                self.cond_depth += (!last) as u32;
                flow = self.exec_pipeline(p);
                self.cond_depth -= (!last) as u32;
            }
        }
        flow
    }

    fn exec_pipeline(&mut self, p: &Pipeline) -> Flow {
        if p.commands.len() == 1 {
            return self.exec_command(&p.commands[0]);
        }
        let started = self.start_pipeline(p);
        let pids: Vec<i32> = started.iter().map(|s| s.0).collect();
        self.host.foreground(&pids);
        for (pid, name, last) in &started {
            let code = self.host.wait(*pid);
            if *last {
                self.status = code;
                self.report(name, code);
            }
        }
        self.host.foreground(&[]);
        if started.last().is_none_or(|s| !s.2) && self.status == 0 {
            self.status = NOT_FOUND;
        }
        if self.check_interrupt() { Flow::Stop } else { Flow::Next }
    }

    fn report(&mut self, name: &str, code: i32) {
        if self.interactive && self.cond_depth == 0 && code != 0 {
            self.err(&format!("[{name} exited with {code}]"));
        }
    }

    /// `&`: パイプの全部を起動して待たない
    fn background(&mut self, item: &Item) -> Flow {
        let ao = &item.and_or;
        let started = if ao.rest.is_empty() {
            self.start_pipeline(&ao.first)
        } else {
            // `a && b &` は全体を子の sh で
            let start = ao.first.commands[0].span.0;
            let end = ao.rest.last().unwrap().1.commands.last().unwrap().span.1;
            let src = String::from(&self.src[start..end]);
            let name = String::from("sh");
            self.spawn_sh(&src, [-1; 3]).map(|pid| alloc::vec![(pid, name, true)]).unwrap_or_default()
        };
        for (pid, name, _) in started {
            self.host.detach(pid);
            if self.interactive {
                self.err(&format!("[{pid}] {name}"));
            }
        }
        self.status = 0;
        Flow::Next
    }

    /// パイプでつないで起動する。(ID, 名前, 最後のコマンドか)
    fn start_pipeline(&mut self, p: &Pipeline) -> Vec<(i32, String, bool)> {
        let n = p.commands.len();
        let mut started = Vec::new();
        let mut prev_read: Option<i32> = None;
        for (i, cmd) in p.commands.iter().enumerate() {
            let last = i + 1 == n;
            let (next_read, write) = if last {
                (None, None)
            } else {
                match self.host.pipe() {
                    Ok((r, w)) => (Some(r), Some(w)),
                    Err(e) => {
                        self.err(&format!("sh: pipe: {}", err::name(e)));
                        break;
                    }
                }
            };
            let fds = [prev_read.unwrap_or(-1), write.unwrap_or(-1), -1];
            if let Some((pid, name)) = self.spawn_command(cmd, fds) {
                started.push((pid, name, last));
            }
            for f in write.into_iter().chain(prev_read) {
                self.host.close(f);
            }
            prev_read = next_read;
        }
        if let Some(r) = prev_read {
            self.host.close(r);
        }
        started
    }

    /// リダイレクトを開いて fd 0〜2 の行き先を決める（-1 はそのまま）。開いた fd も返す
    fn open_redirects(&mut self, redirects: &[Redir], mut fds: [i32; 3]) -> Result<([i32; 3], Vec<i32>), ()> {
        let mut opened = Vec::new();
        for r in redirects {
            let fd = r.fd as usize;
            if r.op == RedirOp::Dup {
                let m: usize = self.expand_str(&r.target).parse().unwrap_or(1);
                fds[fd] = if fds[m] < 0 { m as i32 } else { fds[m] };
                continue;
            }
            let path = self.expand_str(&r.target);
            let flags = match r.op {
                RedirOp::Write => OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE,
                RedirOp::Append => OPEN_WRITE | OPEN_CREATE | OPEN_APPEND,
                _ => 0,
            };
            match self.host.open(&path, flags) {
                Ok(f) => {
                    opened.push(f);
                    fds[fd] = f;
                }
                Err(e) => {
                    self.err(&format!("{path}: {}", err::name(e)));
                    for f in opened {
                        self.host.close(f);
                    }
                    return Err(());
                }
            }
        }
        Ok((fds, opened))
    }

    /// 子の sh で `src` を動かす（位置引数とシェルの変数も渡す）
    fn spawn_sh(&mut self, src: &str, fds: [i32; 3]) -> Option<i32> {
        let mut argv = alloc::vec![String::from("sh"), String::from("-c"), String::from(src), self.arg0.clone()];
        argv.extend(self.args.iter().cloned());
        let env = self.environment(true, &[]);
        match self.host.spawn(&argv, &env, fds) {
            Ok(pid) => Some(pid),
            Err(e) => {
                self.err(&format!("sh: {}", err::name(e)));
                None
            }
        }
    }

    /// パイプの中のコマンドを 1 つ起動する。(ID, 名前)
    fn spawn_command(&mut self, cmd: &Command, fds: [i32; 3]) -> Option<(i32, String)> {
        let (fds, opened) = self.open_redirects(&cmd.redirects, fds).ok()?;
        let r = match &cmd.kind {
            Kind::Simple { assigns, words } => {
                let argv = self.expand_words(words);
                let extra: Vec<(String, String)> =
                    assigns.iter().map(|(k, w)| (k.clone(), self.expand_str(w))).collect();
                match argv.first() {
                    None => None,
                    Some(name) if is_lang_builtin(name) || self.host.is_builtin(name) => {
                        let name = name.clone();
                        self.spawn_sh(&quote(&argv), fds).map(|pid| (pid, name))
                    }
                    Some(name) => {
                        let env = self.environment(false, &extra);
                        match self.host.spawn(&argv, &env, fds) {
                            Ok(pid) => Some((pid, name.clone())),
                            Err(e) => {
                                self.not_started(name, e);
                                None
                            }
                        }
                    }
                }
            }
            _ => {
                let src = String::from(&self.src[cmd.span.0..cmd.span.1]);
                self.spawn_sh(&src, fds).map(|pid| (pid, String::from("sh")))
            }
        };
        for f in opened {
            self.host.close(f);
        }
        r
    }

    fn not_started(&mut self, name: &str, e: i32) {
        if e == err::ENOENT {
            self.err(&format!("unknown command: {name}"));
        } else {
            self.err(&format!("{name}: {}", err::name(e)));
        }
        self.status = NOT_FOUND;
    }

    /// 1 つのコマンドを前面で動かす
    fn exec_command(&mut self, cmd: &Command) -> Flow {
        let Kind::Simple { assigns, words } = &cmd.kind else {
            // 複合コマンド: リダイレクトの間だけ fd 0〜2 をつなぎ替えて、このプロセスの中で
            let Ok((fds, opened)) = self.open_redirects(&cmd.redirects, [-1; 3]) else {
                self.status = 1;
                return Flow::Next;
            };
            let saved = self.host.redirect(fds);
            let flow = self.exec_compound(&cmd.kind);
            self.host.restore(saved);
            for f in opened {
                self.host.close(f);
            }
            return flow;
        };
        let argv = self.expand_words(words);
        let extra: Vec<(String, String)> = assigns.iter().map(|(k, w)| (k.clone(), self.expand_str(w))).collect();
        let Some(name) = argv.first().cloned() else {
            // 代入だけ（リダイレクトはファイルを作る・切り詰めるだけ）
            for (k, v) in &extra {
                self.set_var(k, v);
            }
            self.status = match self.open_redirects(&cmd.redirects, [-1; 3]) {
                Ok((_, opened)) => {
                    for f in opened {
                        self.host.close(f);
                    }
                    0
                }
                Err(()) => 1,
            };
            return Flow::Next;
        };
        let Ok((fds, opened)) = self.open_redirects(&cmd.redirects, [-1; 3]) else {
            self.status = 1;
            return Flow::Next;
        };
        let flow = if is_lang_builtin(&name) || self.host.is_builtin(&name) {
            let saved = self.host.redirect(fds);
            let flow = if is_lang_builtin(&name) {
                self.lang_builtin(&argv)
            } else {
                self.status = self.host.builtin(&argv);
                Flow::Next
            };
            self.host.restore(saved);
            flow
        } else {
            let env = self.environment(false, &extra);
            match self.host.spawn(&argv, &env, fds) {
                Ok(pid) => {
                    self.host.foreground(&[pid]);
                    let code = self.host.wait(pid);
                    self.host.foreground(&[]);
                    self.status = code;
                    self.report(&name, code);
                }
                Err(e) => self.not_started(&name, e),
            }
            Flow::Next
        };
        for f in opened {
            self.host.close(f);
        }
        if flow == Flow::Next && self.check_interrupt() { Flow::Stop } else { flow }
    }

    fn exec_compound(&mut self, kind: &Kind) -> Flow {
        match kind {
            Kind::Simple { .. } => unreachable!(),
            Kind::If { arms, otherwise } => {
                for (cond, body) in arms {
                    self.cond_depth += 1;
                    let flow = self.exec_list(cond);
                    self.cond_depth -= 1;
                    if flow != Flow::Next {
                        return flow;
                    }
                    if self.status == 0 {
                        return self.exec_list(body);
                    }
                }
                match otherwise {
                    Some(body) => self.exec_list(body),
                    None => {
                        self.status = 0;
                        Flow::Next
                    }
                }
            }
            Kind::While { cond, body } => {
                let mut status = 0;
                loop {
                    if self.check_interrupt() {
                        return Flow::Stop;
                    }
                    self.cond_depth += 1;
                    let flow = self.exec_list(cond);
                    self.cond_depth -= 1;
                    if flow != Flow::Next {
                        return flow;
                    }
                    if self.status != 0 {
                        break;
                    }
                    match self.loop_body(body) {
                        Some(Flow::Next) => {}
                        Some(f) => return f,
                        None => break,
                    }
                    status = self.status;
                }
                self.status = status;
                Flow::Next
            }
            Kind::For { var, items, body } => {
                let values = match items {
                    Some(words) => self.expand_words(words),
                    None => self.args.clone(),
                };
                self.status = 0;
                for v in values {
                    if self.check_interrupt() {
                        return Flow::Stop;
                    }
                    self.set_var(var, &v);
                    match self.loop_body(body) {
                        Some(Flow::Next) => {}
                        Some(f) => return f,
                        None => break,
                    }
                }
                Flow::Next
            }
        }
    }

    /// 繰り返しの中身を 1 回。None はこの繰り返しを抜ける。Some(Next) は続ける
    fn loop_body(&mut self, body: &List) -> Option<Flow> {
        self.loop_depth += 1;
        let flow = self.exec_list(body);
        self.loop_depth -= 1;
        match flow {
            Flow::Break(1) => None,
            Flow::Break(n) => Some(Flow::Break(n - 1)),
            Flow::Continue(1) | Flow::Next => Some(Flow::Next),
            Flow::Continue(n) => Some(Flow::Continue(n - 1)),
            Flow::Stop => Some(Flow::Stop),
        }
    }

    // --- 展開 -------------------------------------------------------------------

    /// 語を展開して引数の並びにする（空白で分ける・ファイル名の展開）
    pub fn expand_words(&mut self, words: &[Word]) -> Vec<String> {
        let mut out = Vec::new();
        for w in words {
            self.expand_fields(w, &mut out);
        }
        out
    }

    /// 1 つの文字列に展開する（分けない・ファイル名の展開をしない。代入の値やリダイレクトの先）
    pub fn expand_str(&mut self, w: &Word) -> String {
        let mut s = String::new();
        for p in w {
            match p {
                Part::Lit(t, _) => s.push_str(t),
                _ => s.push_str(&self.part_value(p)),
            }
        }
        s
    }

    fn part_value(&mut self, p: &Part) -> String {
        match p {
            Part::Lit(t, _) => t.clone(),
            Part::Var(n, _) => self.var(n).unwrap_or_default(),
            Part::Arith(e, _) => {
                let text = self.expand_dollars(e);
                let vars = &self.vars;
                match arith::eval(&text, |name| vars.get(name).map_or(0, |v| arith::number(&v.value))) {
                    Ok(v) => v.to_string(),
                    Err(e) => {
                        self.err(&format!("sh: $(({text})): {e}"));
                        String::from("0")
                    }
                }
            }
            Part::Cmd(src, _) => self.command_output(src),
        }
    }

    /// 式の中の `$X`・`${X}`・`$?` などを値にする（`$((…))` の中身用）
    fn expand_dollars(&mut self, text: &str) -> String {
        match crate::lex::lex(&format!("\"{}\"", text.replace('"', "\\\""))) {
            Ok(toks) => match toks.first().map(|t| &t.tok) {
                Some(crate::lex::Tok::Word(w)) => {
                    let w = w.clone();
                    self.expand_str(&w)
                }
                _ => String::from(text),
            },
            Err(_) => String::from(text),
        }
    }

    /// `$(cmd)`: 子の sh で動かして、標準出力（最後の改行は除く）
    fn command_output(&mut self, src: &str) -> String {
        let Ok((r, w)) = self.host.pipe() else { return String::new() };
        let pid = self.spawn_sh(src, [-1, w, -1]);
        self.host.close(w);
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = self.host.read(r, &mut buf);
            if n <= 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
        }
        self.host.close(r);
        if let Some(pid) = pid {
            self.status = self.host.wait(pid);
        }
        let mut s = String::from_utf8_lossy(&out).into_owned();
        while s.ends_with('\n') {
            s.pop();
        }
        s
    }

    fn expand_fields(&mut self, w: &Word, out: &mut Vec<String>) {
        let mut f = Fields::default();
        for p in w {
            match p {
                Part::Lit(t, quoted) => f.add(t, *quoted),
                Part::Var(n, true) if n == "@" => {
                    // "$@": 引数ごとに別の語（引数が無ければ語にしない）
                    let args = self.args.clone();
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            f.finish(self, out);
                        }
                        f.add(a, true);
                    }
                }
                Part::Var(_, q) | Part::Arith(_, q) | Part::Cmd(_, q) => {
                    let v = self.part_value(p);
                    if *q {
                        f.add(&v, true);
                    } else {
                        // 空白で分ける
                        if v.starts_with(char::is_whitespace) {
                            f.finish(self, out);
                        }
                        let mut first = true;
                        for word in v.split_whitespace() {
                            if !first {
                                f.finish(self, out);
                            }
                            f.add(word, false);
                            first = false;
                        }
                        if v.ends_with(char::is_whitespace) {
                            f.finish(self, out);
                        }
                    }
                }
            }
        }
        f.finish(self, out);
    }

    /// ファイル名の展開（カレントディレクトリの中だけ。名前の順。合うものが無ければ None）
    fn glob(&mut self, pattern: &str) -> Option<Vec<String>> {
        if pattern.contains('/') {
            return None;
        }
        let mut names: Vec<String> = self
            .host
            .list_dir()
            .into_iter()
            .filter(|n| (!n.starts_with('.') || pattern.starts_with('.')) && glob_match(pattern, n))
            .collect();
        if names.is_empty() {
            return None;
        }
        // 名前の順（数は少ないので挿入ソート。標準の sort はアプリを大きくする）
        for i in 1..names.len() {
            let mut j = i;
            while j > 0 && names[j - 1] > names[j] {
                names.swap(j - 1, j);
                j -= 1;
            }
        }
        Some(names)
    }

    // --- 言語の内蔵コマンド ---------------------------------------------------------

    fn out(&mut self, s: &str) {
        self.host.write(1, s.as_bytes());
    }

    fn lang_builtin(&mut self, argv: &[String]) -> Flow {
        let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        let mut flow = Flow::Next;
        self.status = match argv[0].as_str() {
            "echo" => {
                let (newline, words) = match args.first() {
                    Some(&"-n") => (false, &args[1..]),
                    _ => (true, &args[..]),
                };
                let mut s = words.join(" ");
                if newline {
                    s.push('\n');
                }
                self.out(&s);
                0
            }
            "true" | ":" => 0,
            "false" => 1,
            "test" => self.test(&args),
            "[" => match args.split_last() {
                Some((&"]", rest)) => self.test(rest),
                _ => {
                    self.err("[: missing ]");
                    2
                }
            },
            "export" => {
                if args.is_empty() {
                    for e in self.environment(false, &[]) {
                        self.out(&format!("{e}\n"));
                    }
                }
                let mut code = 0;
                for a in args {
                    let (name, value) = match a.split_once('=') {
                        Some((n, v)) => (n, Some(v)),
                        None => (a, None),
                    };
                    if is_name(name) {
                        self.export(name, value);
                    } else {
                        self.err(&format!("export: bad name: {name}"));
                        code = 1;
                    }
                }
                code
            }
            "unset" => {
                for a in args {
                    self.vars.remove(a);
                }
                0
            }
            "set" => {
                let list: Vec<String> = self.vars.iter().map(|(k, v)| format!("{k}={}\n", v.value)).collect();
                for l in list {
                    self.out(&l);
                }
                0
            }
            "env" => {
                for e in self.environment(false, &[]) {
                    self.out(&format!("{e}\n"));
                }
                0
            }
            "shift" => {
                let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
                if n > self.args.len() {
                    1
                } else {
                    self.args.drain(..n);
                    0
                }
            }
            "." => match args.first() {
                None => {
                    self.err(".: usage: . file [args...]");
                    2
                }
                Some(path) => match self.host.read_file(path) {
                    Ok(data) => {
                        let src = String::from_utf8_lossy(&data).into_owned();
                        let saved = (!args[1..].is_empty())
                            .then(|| core::mem::replace(&mut self.args, args[1..].iter().map(|s| s.to_string()).collect()));
                        self.run(&src);
                        if let Some(a) = saved {
                            self.args = a;
                        }
                        if self.exit.is_some() {
                            flow = Flow::Stop;
                        }
                        self.status
                    }
                    Err(e) => {
                        self.err(&format!(".: {path}: {}", err::name(e)));
                        1
                    }
                },
            },
            "exit" => {
                let code = match args.first().map(|s| s.parse::<i32>()) {
                    None => self.status,
                    Some(Ok(c)) => c,
                    Some(Err(_)) => {
                        self.err("exit: bad number");
                        2
                    }
                };
                self.exit = Some(code);
                flow = Flow::Stop;
                code
            }
            b @ ("break" | "continue") => {
                let n: u32 = args.first().and_then(|s| s.parse().ok()).filter(|&n| n > 0).unwrap_or(1);
                if self.loop_depth == 0 {
                    self.err(&format!("{b}: only in a loop"));
                    1
                } else {
                    flow = if b == "break" { Flow::Break(n) } else { Flow::Continue(n) };
                    0
                }
            }
            _ => unreachable!(),
        };
        flow
    }

    /// test の式。真なら 0、偽なら 1、誤りなら 2
    fn test(&mut self, args: &[&str]) -> i32 {
        match self.test_expr(args) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(msg) => {
                self.err(&format!("test: {msg}"));
                2
            }
        }
    }

    fn test_expr(&mut self, args: &[&str]) -> Result<bool, String> {
        let int = |s: &str| s.trim().parse::<i64>().map_err(|_| format!("not a number: {s}"));
        Ok(match args {
            [] => false,
            ["!", rest @ ..] => !self.test_expr(rest)?,
            [s] => !s.is_empty(),
            ["-z", s] => s.is_empty(),
            ["-n", s] => !s.is_empty(),
            ["-e", p] => self.host.stat(p).is_some(),
            ["-f", p] => self.host.stat(p) == Some(false),
            ["-d", p] => self.host.stat(p) == Some(true),
            [a, "=", b] => a == b,
            [a, "!=", b] => a != b,
            [a, op, b] => {
                let (x, y) = (int(a)?, int(b)?);
                match *op {
                    "-eq" => x == y,
                    "-ne" => x != y,
                    "-lt" => x < y,
                    "-le" => x <= y,
                    "-gt" => x > y,
                    "-ge" => x >= y,
                    _ => return Err(format!("unknown operator: {op}")),
                }
            }
            _ => return Err(String::from("too many arguments")),
        })
    }
}

/// 展開の途中の語
#[derive(Default)]
struct Fields {
    text: String,
    /// ファイル名の展開の型（引用した `*?\` は `\` を付ける）
    pattern: String,
    /// 引用していない `*` か `?` がある
    glob: bool,
    /// 語がある（空の "" でも語になる）
    any: bool,
}

impl Fields {
    fn add(&mut self, s: &str, quoted: bool) {
        self.any = true;
        self.text.push_str(s);
        for c in s.chars() {
            if quoted && matches!(c, '*' | '?' | '\\') {
                self.pattern.push('\\');
            } else if !quoted && matches!(c, '*' | '?') {
                self.glob = true;
            }
            self.pattern.push(c);
        }
    }

    fn finish<H: Host>(&mut self, sh: &mut Shell<H>, out: &mut Vec<String>) {
        let f = core::mem::take(self);
        if !f.any {
            return;
        }
        match f.glob.then(|| sh.glob(&f.pattern)).flatten() {
            Some(names) => out.extend(names),
            None => out.push(f.text),
        }
    }
}

/// ファイル名の型に合うか（`*` は何文字でも、`?` は 1 文字、`\` の後はその文字。大文字と小文字は区別しない）
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some(('?', rest)) => !n.is_empty() && go(rest, &n[1..]),
            Some(('\\', rest)) if !rest.is_empty() => {
                !n.is_empty() && n[0].eq_ignore_ascii_case(&rest[0]) && go(&rest[1..], &n[1..])
            }
            Some((c, rest)) => !n.is_empty() && n[0].eq_ignore_ascii_case(c) && go(rest, &n[1..]),
        }
    }
    go(&p, &n)
}

/// `sh -c` に渡せるように引用する（'...' の中の ' は '\'' にする）
pub fn quote(argv: &[String]) -> String {
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

/// 1 行が途中で終わっている（続きの行を読む）か
pub fn is_incomplete(src: &str) -> bool {
    matches!(parse_script(src), Err(ParseError::Incomplete(_)))
}
