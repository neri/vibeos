//! vibe-run: QEMU を起動し、シナリオファイルどおりに操作・検証するテストランナー。
//!
//! 使い方:
//!   vibe-run test [--img PATH] [--out DIR] SCENARIO...
//!
//! シナリオファイルは 1 行 1 コマンド（`#` 以降はコメント）:
//!   image PATH            起動するイメージ（既定は --img の値）
//!   qemu-arg ARG          QEMU に渡す追加引数（起動前に集められる）
//!   timeout SECS          以降の待ち系コマンドのタイムアウト（既定 10 秒）
//!   wait-serial TEXT      シリアル出力に TEXT が現れるまで待つ（前回一致した位置より後ろを探す）
//!   serial-absent TEXT    これまでのシリアル出力全体に TEXT が無いことを確認する
//!   sleep MS              待つ
//!   expect-running        QEMU がまだ動いていることを確認する
//!   screendump NAME       画面を OUT/NAME.png に保存する
//!   expect-screen NAME [X Y W H]
//!                         画面を tests/golden/NAME.png と比較する。矩形を指定するとその範囲だけ
//!                         切り出して比較する（UPDATE_GOLDEN=1 のときは期待画像を更新する）
//!   expect-pixel X Y R G B  画面の 1 点の色を確認する
//!   expect-change X Y W H MS  矩形の中身が MS ミリ秒の間に変わることを確認する（動き続けているものの確認）
//!   key KEYS              キーを押す（例: `key a`, `key shift+a`, `key ret`。QEMU の qcode 名）
//!   type TEXT             文字列を US 配列で打つ
//!   mouse-move DX DY      マウスを相対的に動かす（右・下が正）
//!   mouse-button BTN up|down  マウスのボタン（left / right / middle）
//!   expect-exit CODE      QEMU が終了コード CODE で終わるのを待つ
//!                         （isa-debug-exit に v を書くと終了コードは (v << 1) | 1）
//!
//! 環境変数: QEMU（既定 qemu-system-i386）, UPDATE_GOLDEN, VIBE_RUN_VERBOSE（シリアルをそのまま表示）

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

type Result<T> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("test") => run_tests(&args[1..]),
        _ => {
            eprintln!("usage: vibe-run test [--img PATH] [--out DIR] SCENARIO...");
            ExitCode::from(2)
        }
    }
}

fn run_tests(args: &[String]) -> ExitCode {
    let mut img = PathBuf::from("build/vibeos.img");
    let mut out = PathBuf::from("build/test-out");
    let mut scenarios = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--img" => img = it.next().expect("--img needs a value").into(),
            "--out" => out = it.next().expect("--out needs a value").into(),
            _ => scenarios.push(PathBuf::from(a)),
        }
    }
    if scenarios.is_empty() {
        eprintln!("no scenarios given");
        return ExitCode::from(2);
    }
    fs::create_dir_all(&out).expect("create output dir");

    let mut failed = 0;
    for scn in &scenarios {
        let name = scn.file_stem().unwrap().to_string_lossy().into_owned();
        let started = Instant::now();
        match run_scenario(scn, &name, &img, &out) {
            Ok(()) => println!("PASS {name} ({:.1}s)", started.elapsed().as_secs_f32()),
            Err(e) => {
                failed += 1;
                println!("FAIL {name}: {e}");
            }
        }
    }
    println!("{} passed, {} failed", scenarios.len() - failed, failed);
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

struct Step {
    line: usize,
    cmd: String,
    arg: String,
}

fn parse_scenario(path: &Path) -> Result<Vec<Step>> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut steps = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = match raw.find(" #") {
            Some(p) => &raw[..p],
            None if raw.trim_start().starts_with('#') => "",
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let (cmd, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        steps.push(Step { line: i + 1, cmd: cmd.to_string(), arg: arg.trim().to_string() });
    }
    Ok(steps)
}

fn run_scenario(path: &Path, name: &str, default_img: &Path, out: &Path) -> Result<()> {
    let steps = parse_scenario(path)?;
    let mut img = default_img.to_path_buf();
    let mut extra = Vec::new();
    for s in &steps {
        match s.cmd.as_str() {
            "image" => img = PathBuf::from(&s.arg),
            "qemu-arg" => extra.push(s.arg.clone()),
            _ => {}
        }
    }
    if !img.exists() {
        return Err(format!("image not found: {}", img.display()));
    }

    let mut vm = Vm::start(&img, &extra, out, name)?;
    let result = run_steps(&mut vm, &steps, name, out);
    if result.is_err() {
        let _ = vm.screendump(&out.join(format!("{name}-fail.png")));
    }
    vm.shutdown();
    let serial = vm.serial_text();
    let _ = fs::write(out.join(format!("{name}.serial.log")), &serial);
    result.map_err(|e| {
        let tail: Vec<&str> = serial.lines().rev().take(15).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        format!("{e}\n--- serial (last lines) ---\n{}\n---", tail.join("\n"))
    })
}

fn run_steps(vm: &mut Vm, steps: &[Step], name: &str, out: &Path) -> Result<()> {
    let mut timeout = Duration::from_secs(10);
    for s in steps {
        let at = |e: String| format!("line {} `{} {}`: {e}", s.line, s.cmd, s.arg);
        match s.cmd.as_str() {
            "image" | "qemu-arg" => {}
            "timeout" => timeout = Duration::from_secs_f32(s.arg.parse().map_err(|_| at("bad number".into()))?),
            "wait-serial" => vm.wait_serial(&s.arg, timeout).map_err(at)?,
            "serial-absent" => {
                if vm.serial_text().contains(&s.arg) {
                    return Err(at("text found in serial output".into()));
                }
            }
            "sleep" => thread::sleep(Duration::from_millis(s.arg.parse().map_err(|_| at("bad number".into()))?)),
            "expect-running" => vm.expect_running().map_err(at)?,
            "screendump" => vm.screendump(&out.join(format!("{}.png", s.arg))).map_err(at)?,
            "expect-screen" => {
                let mut words = s.arg.split_whitespace();
                let golden_name = words.next().ok_or_else(|| at("missing name".into()))?;
                let rect: Vec<u32> = words.map(|w| w.parse().unwrap_or(u32::MAX)).collect();
                let rect = match rect.as_slice() {
                    [] => None,
                    [x, y, w, h] => Some((*x, *y, *w, *h)),
                    _ => return Err(at("expected NAME [X Y W H]".into())),
                };
                let shot = out.join(format!("{name}-{golden_name}.png"));
                vm.screendump(&shot).map_err(at)?;
                if let Some((x, y, w, h)) = rect {
                    Image::load(&shot).and_then(|i| i.crop(x, y, w, h)).and_then(|i| i.save(&shot)).map_err(at)?;
                }
                compare_golden(&shot, &Path::new("tests/golden").join(format!("{golden_name}.png"))).map_err(at)?;
            }
            "expect-pixel" => {
                let v: Vec<u32> = s.arg.split_whitespace().map(|x| x.parse().unwrap_or(u32::MAX)).collect();
                if v.len() != 5 {
                    return Err(at("expected X Y R G B".into()));
                }
                let shot = out.join(format!("{name}-pixel.png"));
                vm.screendump(&shot).map_err(at)?;
                let img = Image::load(&shot).map_err(at)?;
                let got = img.pixel(v[0], v[1]).ok_or_else(|| at("pixel out of range".into()))?;
                let want = [v[2] as u8, v[3] as u8, v[4] as u8];
                if got != want {
                    return Err(at(format!("pixel is {got:?}, want {want:?}")));
                }
            }
            "expect-change" => {
                let v: Vec<u32> = s.arg.split_whitespace().map(|x| x.parse().unwrap_or(u32::MAX)).collect();
                let [x, y, w, h, ms] = v[..] else {
                    return Err(at("expected X Y W H MS".into()));
                };
                let a_path = out.join(format!("{name}-change-a.png"));
                let b_path = out.join(format!("{name}-change-b.png"));
                vm.screendump(&a_path).map_err(at)?;
                thread::sleep(Duration::from_millis(ms as u64));
                vm.screendump(&b_path).map_err(at)?;
                let a = Image::load(&a_path).and_then(|i| i.crop(x, y, w, h)).map_err(at)?;
                let b = Image::load(&b_path).and_then(|i| i.crop(x, y, w, h)).map_err(at)?;
                if a.rgb == b.rgb {
                    return Err(at(format!("region did not change in {ms} ms")));
                }
            }
            "mouse-move" => {
                let v: Vec<i64> = s.arg.split_whitespace().map(|x| x.parse().unwrap_or(i64::MAX)).collect();
                let [dx, dy] = v[..] else {
                    return Err(at("expected DX DY".into()));
                };
                vm.send_input(json!([
                    {"type": "rel", "data": {"axis": "x", "value": dx}},
                    {"type": "rel", "data": {"axis": "y", "value": dy}},
                ]))
                .map_err(at)?;
                thread::sleep(Duration::from_millis(50));
            }
            "mouse-button" => {
                let mut w = s.arg.split_whitespace();
                let (Some(btn), Some(state)) = (w.next(), w.next()) else {
                    return Err(at("expected BTN up|down".into()));
                };
                vm.send_input(json!([{"type": "btn", "data": {"button": btn, "down": state == "down"}}])).map_err(at)?;
                thread::sleep(Duration::from_millis(50));
            }
            "key" => vm.send_key(&s.arg.split('+').collect::<Vec<_>>()).map_err(at)?,
            "type" => {
                for c in s.arg.chars() {
                    let keys = qcodes_for_char(c).ok_or_else(|| at(format!("cannot type {c:?}")))?;
                    vm.send_key(&keys).map_err(at)?;
                    thread::sleep(Duration::from_millis(30));
                }
            }
            "expect-exit" => {
                let want: i32 = s.arg.parse().map_err(|_| at("bad number".into()))?;
                let got = vm.wait_exit(timeout).map_err(at)?;
                if got != want {
                    return Err(at(format!("exit code {got}, want {want}")));
                }
            }
            other => return Err(at(format!("unknown command {other}"))),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// QEMU の起動と操作

struct Vm {
    child: Child,
    qmp: Option<Qmp>,
    serial: Arc<Mutex<Vec<u8>>>,
    serial_pos: usize,
    sock: PathBuf,
    exit_code: Option<i32>,
}

impl Vm {
    fn start(img: &Path, extra: &[String], out: &Path, name: &str) -> Result<Vm> {
        let sock = std::env::temp_dir().join(format!("vibe-run-{}-{name}.qmp", std::process::id()));
        let _ = fs::remove_file(&sock);
        let qemu = std::env::var("QEMU").unwrap_or_else(|_| "qemu-system-i386".into());
        let mut cmd = Command::new(&qemu);
        cmd.args(["-m", "32M", "-cpu", "pentium2"])
            .arg("-drive")
            .arg(format!("file={},format=raw,if=floppy,snapshot=on", img.display()))
            .args(["-boot", "a", "-vga", "std", "-nic", "none", "-display", "none", "-serial", "stdio", "-monitor", "none"])
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", sock.display()))
            .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04", "-no-reboot"])
            .args(extra)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                fs::File::create(out.join(format!("{name}.qemu.log"))).map_err(|e| e.to_string())?,
            ));
        let mut child = cmd.spawn().map_err(|e| format!("cannot start {qemu}: {e}"))?;

        let serial = Arc::new(Mutex::new(Vec::new()));
        let mut stdout = child.stdout.take().unwrap();
        let buf = serial.clone();
        let verbose = std::env::var_os("VIBE_RUN_VERBOSE").is_some();
        thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if verbose {
                    let _ = std::io::stderr().write_all(&chunk[..n]);
                }
                buf.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });

        let mut vm = Vm { child, qmp: None, serial, serial_pos: 0, sock, exit_code: None };
        vm.qmp = Some(Qmp::connect(&vm.sock, Duration::from_secs(5))?);
        Ok(vm)
    }

    fn serial_text(&self) -> String {
        String::from_utf8_lossy(&self.serial.lock().unwrap()).into_owned()
    }

    fn exited(&mut self) -> Option<i32> {
        if self.exit_code.is_none() {
            if let Ok(Some(st)) = self.child.try_wait() {
                self.exit_code = Some(st.code().unwrap_or(-1));
            }
        }
        self.exit_code
    }

    fn wait_serial(&mut self, text: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let buf = self.serial.lock().unwrap();
                let hay = &buf[self.serial_pos.min(buf.len())..];
                if let Some(p) = find(hay, text.as_bytes()) {
                    self.serial_pos += p + text.len();
                    return Ok(());
                }
            }
            if let Some(code) = self.exited() {
                // QEMU 終了直後に読み残しがあるかもしれないので少し待ってもう一度だけ見る
                thread::sleep(Duration::from_millis(100));
                let buf = self.serial.lock().unwrap();
                if find(&buf[self.serial_pos.min(buf.len())..], text.as_bytes()).is_some() {
                    return Ok(());
                }
                return Err(format!("QEMU exited (code {code}) before the text appeared"));
            }
            if Instant::now() > deadline {
                return Err("timed out waiting for serial text".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_exit(&mut self, timeout: Duration) -> Result<i32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(code) = self.exited() {
                return Ok(code);
            }
            if Instant::now() > deadline {
                return Err("timed out waiting for QEMU to exit".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn qmp(&mut self) -> Result<&mut Qmp> {
        if let Some(code) = self.exited() {
            return Err(format!("QEMU already exited (code {code})"));
        }
        Ok(self.qmp.as_mut().unwrap())
    }

    fn expect_running(&mut self) -> Result<()> {
        let st = self.qmp()?.execute("query-status", json!({}))?;
        if st["running"] == json!(true) { Ok(()) } else { Err(format!("VM not running: {st}")) }
    }

    fn screendump(&mut self, path: &Path) -> Result<()> {
        let abs = std::env::current_dir().unwrap().join(path);
        self.qmp()?.execute("screendump", json!({"filename": abs.to_string_lossy(), "format": "png"}))?;
        Ok(())
    }

    fn send_key(&mut self, keys: &[&str]) -> Result<()> {
        let keys: Vec<Value> = keys.iter().map(|k| json!({"type": "qcode", "data": k})).collect();
        self.qmp()?.execute("send-key", json!({ "keys": keys }))?;
        Ok(())
    }

    fn send_input(&mut self, events: Value) -> Result<()> {
        self.qmp()?.execute("input-send-event", json!({ "events": events }))?;
        Ok(())
    }

    fn shutdown(&mut self) {
        if self.exited().is_none() {
            if let Some(q) = self.qmp.as_mut() {
                let _ = q.execute("quit", json!({}));
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.exited().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            if self.exited().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        let _ = fs::remove_file(&self.sock);
        // シリアル読み取りスレッドが残りを読み切るのを少し待つ
        thread::sleep(Duration::from_millis(50));
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

struct Qmp {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Qmp {
    fn connect(path: &Path, timeout: Duration) -> Result<Qmp> {
        let deadline = Instant::now() + timeout;
        let stream = loop {
            match UnixStream::connect(path) {
                Ok(s) => break s,
                Err(e) if Instant::now() > deadline => return Err(format!("cannot connect to QMP: {e}")),
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut q = Qmp { reader: BufReader::new(stream.try_clone().unwrap()), writer: stream };
        q.read_message()?; // greeting
        q.execute("qmp_capabilities", json!({}))?;
        Ok(q)
    }

    fn read_message(&mut self) -> Result<Value> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).map_err(|e| format!("QMP read: {e}"))?;
        if n == 0 {
            return Err("QMP connection closed".into());
        }
        serde_json::from_str(&line).map_err(|e| format!("QMP parse: {e}: {line}"))
    }

    fn execute(&mut self, cmd: &str, args: Value) -> Result<Value> {
        let msg = json!({"execute": cmd, "arguments": args});
        writeln!(self.writer, "{msg}").map_err(|e| format!("QMP write: {e}"))?;
        loop {
            let v = self.read_message()?;
            if let Some(r) = v.get("return") {
                return Ok(r.clone());
            }
            if let Some(e) = v.get("error") {
                return Err(format!("QMP {cmd}: {e}"));
            }
            // それ以外はイベントなので読み飛ばす
        }
    }
}

// ---------------------------------------------------------------------------
// 画像の比較

struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Image {
    fn load(path: &Path) -> Result<Image> {
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let decoder = png::Decoder::new(file);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let data = &buf[..info.buffer_size()];
        let rgb = match info.color_type {
            png::ColorType::Rgb => data.to_vec(),
            png::ColorType::Rgba => data.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
            other => return Err(format!("unsupported PNG color type {other:?}")),
        };
        Ok(Image { width: info.width, height: info.height, rgb })
    }

    fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Result<Image> {
        if x + w > self.width || y + h > self.height {
            return Err(format!("crop {x},{y} {w}x{h} outside {}x{}", self.width, self.height));
        }
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for row in y..y + h {
            let start = ((row * self.width + x) * 3) as usize;
            rgb.extend_from_slice(&self.rgb[start..start + (w * 3) as usize]);
        }
        Ok(Image { width: w, height: h, rgb })
    }

    fn save(&self, path: &Path) -> Result<()> {
        let file = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&self.rgb).map_err(|e| e.to_string())
    }

    fn pixel(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 3) as usize;
        Some([self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]])
    }
}

fn compare_golden(shot: &Path, golden: &Path) -> Result<()> {
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(golden.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(shot, golden).map_err(|e| e.to_string())?;
        eprintln!("updated golden image {}", golden.display());
        return Ok(());
    }
    if !golden.exists() {
        return Err(format!(
            "golden image {} missing (check {} and rerun with UPDATE_GOLDEN=1)",
            golden.display(),
            shot.display()
        ));
    }
    let a = Image::load(shot)?;
    let b = Image::load(golden)?;
    if (a.width, a.height) != (b.width, b.height) {
        return Err(format!("size {}x{} differs from golden {}x{}", a.width, a.height, b.width, b.height));
    }
    let diff = a.rgb.chunks(3).zip(b.rgb.chunks(3)).filter(|(p, q)| p != q).count();
    if diff > 0 {
        return Err(format!("{diff} pixels differ from {} (see {})", golden.display(), shot.display()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 文字 → QEMU の qcode（US 配列）

fn qcodes_for_char(c: char) -> Option<Vec<&'static str>> {
    const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
    const NAMES: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v",
        "w", "x", "y", "z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    if let Some(i) = LOWER.find(c) {
        return Some(vec![NAMES[i]]);
    }
    if c.is_ascii_uppercase() {
        let i = LOWER.find(c.to_ascii_lowercase())?;
        return Some(vec!["shift", NAMES[i]]);
    }
    if let Some(d) = c.to_digit(10) {
        return Some(vec![DIGITS[d as usize]]);
    }
    let plain = |k| Some(vec![k]);
    let shifted = |k| Some(vec!["shift", k]);
    match c {
        ' ' => plain("spc"),
        '\n' => plain("ret"),
        '\t' => plain("tab"),
        '-' => plain("minus"),
        '=' => plain("equal"),
        '[' => plain("bracket_left"),
        ']' => plain("bracket_right"),
        '\\' => plain("backslash"),
        ';' => plain("semicolon"),
        '\'' => plain("apostrophe"),
        '`' => plain("grave_accent"),
        ',' => plain("comma"),
        '.' => plain("dot"),
        '/' => plain("slash"),
        '!' => shifted("1"),
        '@' => shifted("2"),
        '#' => shifted("3"),
        '$' => shifted("4"),
        '%' => shifted("5"),
        '^' => shifted("6"),
        '&' => shifted("7"),
        '*' => shifted("8"),
        '(' => shifted("9"),
        ')' => shifted("0"),
        '_' => shifted("minus"),
        '+' => shifted("equal"),
        '{' => shifted("bracket_left"),
        '}' => shifted("bracket_right"),
        '|' => shifted("backslash"),
        ':' => shifted("semicolon"),
        '"' => shifted("apostrophe"),
        '~' => shifted("grave_accent"),
        '<' => shifted("comma"),
        '>' => shifted("dot"),
        '?' => shifted("slash"),
        _ => None,
    }
}
