//! vibe-run: QEMU を起動し、シナリオファイルどおりに操作・検証するテストランナー。
//!
//! 使い方:
//!   vibe-run test [--img PATH] [--out DIR] SCENARIO...
//!
//! シナリオファイルは 1 行 1 コマンド（`#` 以降はコメント）:
//!   image PATH            起動するイメージ（既定は --img の値）
//!   writable              イメージの写し（OUT/NAME.img）を書き込み可能で使う（既定は書き込みを捨てる）
//!   boot-config TEXT      イメージの写しの BOOT.CFG を TEXT にしてから起動する（例: video=320x200）。
//!                         何行か書くと、その行を並べたものになる
//!   key-hold KEY / key-release KEY  キーを押したまま / 離す（起動中に Shift を押しておくなど）
//!   expect-disk-file PATH TEXT  ディスクのファイル PATH に TEXT が含まれる（mtools で読む。writable のとき）
//!   expect-disk-clean     ディスクに FAT の不整合が無い（fsck.fat -n。writable のとき）
//!   expect-disk-missing PATH  ディスクにファイル PATH が無い（mtools で見る。writable のとき）
//!   quit                  QEMU を終了させる（WAV など終了時に書かれるものを確かめる前に）
//!   expect-wav-sound PATH  WAV ファイルに無音でない部分がある
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
//!   qemu-only             このシナリオは Bochs では動かさない
//!   bochs-only            このシナリオは Bochs でだけ動かす
//!
//! ネットワーク（net.rs。ゲストは slirp の restrict=on の中なので外には出られない）:
//!   net-lan [DEVICE]      net と同じだが slirp を restrict=on にしない（ホストの LAN へ出られる。make test-lan 用。
//!                         WAN へ出ないことはゲストの wan=deny で守り、expect-lan-only で確かめる）
//!   net [DEVICE]          カードを付けて起動する（既定 ne2k_isa,iobase=0x300,irq=10）。フレームは OUT/NAME.pcap に残す
//!   serve-dns NAME=ADDR...  偽のホスト 10.0.2.50 が DNS に答える（ほかの名前は NXDOMAIN）
//!   serve-ntp TIME        偽のホスト 10.0.2.50 が NTP で TIME（UTC, 2026-10-12T01:02:03 か 1970 年からの秒）を返す
//!   serve-http            ゲストの 10.0.2.100:80 への接続に OUT/NAME-www/ のファイルを返す
//!   www-file NAME SIZE    OUT/NAME-www/NAME に SIZE バイトの決まった中身を作る
//!   www-text NAME TEXT    OUT/NAME-www/NAME に TEXT（と改行）を書く
//!   www-repeat NAME SIZE TEXT  OUT/NAME-www/NAME に TEXT を SIZE バイトになるまで繰り返して書く（期待するファイル用）
//!   expect-packet KIND SRC DST  記録したフレームに合うものがある（例: arp-reply 10.0.2.2 10.0.2.15）。
//!                         壊れたフレーム（チェックサムの誤りなど）が 1 つでもあれば失敗。
//!                         SRC・DST の * はどれでもよい
//!   expect-lan-only       ゲストが送った IPv4 のパケットの宛先がすべて LAN の中（WAN へ出ようとしていない）
//!   fake-send arp|ping IP  偽のホストからゲストへ ARP の問い合わせ・ping を送る（返事は expect-packet で見る）
//!   expect-disk-same PATH WWWNAME  ディスクのファイル PATH が OUT/NAME-www/WWWNAME と同じ（writable のとき）
//!   host-get PATH DISKFILE  ホストからゲストの httpd（80 番）に GET し、200 で中身がディスクの DISKFILE と同じ
//!   host-get-status PATH CODE  同じく GET して、ステータスが CODE
//!   host-get-text PATH TEXT  同じく GET して、200 で中身に TEXT が含まれる
//!   host-get-concurrent PATH DISKFILE  2 本同時につないで（1 本目を待たせたまま 2 本目で）取得できる
//!
//! `--bochs` を付けると QEMU の代わりに Bochs で動かす（tools/bochs/）。Bochs では次のものは
//! 飛ばす: 画面の取得と比較・マウス・QEMU 用の引数（qemu-arg）があるシナリオ・音・終了コードの値。
//! 待ち時間の上限は 4 倍にする。
//!
//! 引数の中の ${NAME} は環境変数の値に置き換える（無ければエラー）。
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

mod bochs;
mod net;

type Result<T> = std::result::Result<T, String>;

/// Bochs で動かすときに待ち時間の上限を何倍にするか
const BOCHS_SLOWDOWN: u32 = 4;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("test") => run_tests(&args[1..]),
        // serve-http の guestfwd から接続ごとに起動される
        Some("http-serve") if args.len() == 2 => net::http_serve(Path::new(&args[1])),
        _ => {
            eprintln!("usage: vibe-run test [--bochs] [--img PATH] [--out DIR] SCENARIO...");
            ExitCode::from(2)
        }
    }
}

fn run_tests(args: &[String]) -> ExitCode {
    let mut img = PathBuf::from("build/vibeos.img");
    let mut out = PathBuf::from("build/test-out");
    let mut scenarios = Vec::new();
    let mut use_bochs = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--bochs" => use_bochs = true,
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

    let (mut failed, mut skipped) = (0, 0);
    for scn in &scenarios {
        let name = scn.file_stem().unwrap().to_string_lossy().into_owned();
        let started = Instant::now();
        match run_scenario(scn, &name, &img, &out, use_bochs) {
            Ok(None) => println!("PASS {name} ({:.1}s)", started.elapsed().as_secs_f32()),
            Ok(Some(why)) => {
                skipped += 1;
                println!("SKIP {name} ({why})");
            }
            Err(e) => {
                failed += 1;
                println!("FAIL {name}: {e}");
            }
        }
    }
    println!("{} passed, {} failed, {} skipped", scenarios.len() - failed - skipped, failed, skipped);
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
        let arg = expand_env(arg.trim()).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
        steps.push(Step { line: i + 1, cmd: cmd.to_string(), arg });
    }
    Ok(steps)
}

/// ${NAME} を環境変数の値に置き換える
fn expand_env(s: &str) -> Result<String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find("${") {
        out.push_str(&rest[..p]);
        let end = rest[p..].find('}').ok_or("unterminated ${")? + p;
        let name = &rest[p + 2..end];
        out.push_str(&std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))?);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Bochs では動かせないシナリオか（QEMU の引数・マウス・音・QEMU の終了を使うもの）
fn qemu_only(steps: &[Step]) -> bool {
    steps.iter().any(|s| {
        matches!(
            s.cmd.as_str(),
            "qemu-only" | "qemu-arg" | "mouse-move" | "mouse-button" | "quit" | "expect-wav-sound" | "key-hold" | "net-lan"
        )
    })
}

/// 実行したら Ok(None)、飛ばしたら Ok(Some(理由))
fn run_scenario(path: &Path, name: &str, default_img: &Path, out: &Path, use_bochs: bool) -> Result<Option<&'static str>> {
    let steps = parse_scenario(path)?;
    if use_bochs && qemu_only(&steps) {
        return Ok(Some("QEMU only"));
    }
    if !use_bochs && steps.iter().any(|s| s.cmd == "bochs-only") {
        return Ok(Some("Bochs only"));
    }
    let mut img = default_img.to_path_buf();
    let mut extra = Vec::new();
    let mut writable = false;
    let mut boot_config: Option<String> = None;
    let mut netsetup: Option<net::NetSetup> = None;
    let www = out.join(format!("{name}-www"));
    for s in &steps {
        let at = |e: String| format!("line {} `{} {}`: {e}", s.line, s.cmd, s.arg);
        match s.cmd.as_str() {
            "image" => img = PathBuf::from(&s.arg),
            "qemu-arg" => extra.push(s.arg.clone()),
            "writable" => writable = true,
            // 何行でも書ける（1 行ずつ足す）
            "boot-config" => {
                let cfg = boot_config.get_or_insert_with(String::new);
                cfg.push_str(&s.arg);
                cfg.push('\n');
            }
            "net" | "net-lan" => {
                let n = netsetup.get_or_insert_with(Default::default);
                n.device = if s.arg.is_empty() { "ne2k_isa,iobase=0x300,irq=10".into() } else { s.arg.clone() };
                n.lan = s.cmd == "net-lan";
            }
            "serve-dns" | "serve-ntp" | "serve-http" | "host-get" | "host-get-status" | "host-get-text"
            | "host-get-concurrent" => {
                let n = netsetup.as_mut().ok_or_else(|| at("needs `net` before it".into()))?;
                match s.cmd.as_str() {
                    "serve-dns" => {
                        for pair in s.arg.split_whitespace() {
                            let (k, v) = pair.split_once('=').ok_or_else(|| at("expected NAME=ADDR".into()))?;
                            let a: Vec<u8> = v.split('.').map(|x| x.parse().ok()).collect::<Option<_>>().unwrap_or_default();
                            let a: [u8; 4] = a.try_into().map_err(|_| at(format!("bad address {v}")))?;
                            n.dns.insert(k.to_ascii_lowercase(), a);
                        }
                    }
                    "serve-ntp" => n.ntp = Some(net::parse_time(&s.arg).ok_or_else(|| at("bad time".into()))?),
                    "serve-http" => n.www = Some(www.clone()),
                    _ => n.hostfwd = true,
                }
            }
            "www-file" | "www-text" | "www-repeat" => {
                fs::create_dir_all(&www).map_err(|e| at(e.to_string()))?;
                let (file, rest) = s.arg.split_once(' ').ok_or_else(|| at("expected NAME ...".into()))?;
                let data = if s.cmd == "www-file" {
                    let len: usize = rest.trim().parse().map_err(|_| at("bad size".into()))?;
                    net::pattern_file(len, len as u32 ^ 0x5eed)
                } else if s.cmd == "www-repeat" {
                    let (len, text) = rest.split_once(' ').ok_or_else(|| at("expected NAME SIZE TEXT".into()))?;
                    let len: usize = len.parse().map_err(|_| at("bad size".into()))?;
                    text.bytes().cycle().take(len).collect()
                } else {
                    format!("{rest}\n").into_bytes()
                };
                fs::write(www.join(file), data).map_err(|e| at(e.to_string()))?;
            }
            _ => {}
        }
    }
    if !img.exists() {
        return Err(format!("image not found: {}", img.display()));
    }
    // Bochs はイメージに直接書くので、いつも写しを使う
    if writable || use_bochs || boot_config.is_some() {
        let copy = out.join(format!("{name}.img"));
        fs::copy(&img, &copy).map_err(|e| format!("copy image: {e}"))?;
        img = copy;
    }
    if let Some(text) = &boot_config {
        let cfg = out.join(format!("{name}.boot.cfg"));
        fs::write(&cfg, text).map_err(|e| e.to_string())?;
        let st = Command::new("mcopy")
            .args(["-o", "-i"])
            .arg(&img)
            .arg(&cfg)
            .arg("::BOOT.CFG")
            .status()
            .map_err(|e| format!("mcopy: {e}"))?;
        if !st.success() {
            return Err("cannot write BOOT.CFG".into());
        }
    }

    let mut fake = None;
    if let Some(setup) = netsetup.as_ref().filter(|_| !use_bochs) {
        let (f, args) = net::FakeNet::start(setup, &format!("{}-{name}", std::process::id()))?;
        extra.extend(args);
        fake = Some(f);
    }
    let mut vm = if use_bochs {
        Vm::start_bochs(&img, out, name, netsetup.is_some())?
    } else {
        Vm::start(&img, &extra, out, name, !writable)?
    };
    vm.disk = img.clone();
    vm.net = fake;
    vm.www = www;
    let result = run_steps(&mut vm, &steps, name, out);
    if result.is_err() && !vm.is_bochs() {
        let _ = vm.screendump(&out.join(format!("{name}-fail.png")));
    }
    vm.shutdown();
    if let Some(n) = &vm.net {
        n.finish(&out.join(format!("{name}.pcap")));
    }
    let serial = vm.serial_text();
    let _ = fs::write(out.join(format!("{name}.serial.log")), &serial);
    result.map(|_| None).map_err(|e| {
        let tail: Vec<&str> = serial.lines().rev().take(15).collect();
        let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        // 改行を出さない画面描画（エディタなど）で長くなりすぎないよう、最後の 2000 文字まで
        let start = tail.char_indices().rev().nth(2000).map_or(0, |(i, _)| i);
        format!("{e}\n--- serial (last lines) ---\n{}\n---", &tail[start..])
    })
}

fn run_steps(vm: &mut Vm, steps: &[Step], name: &str, out: &Path) -> Result<()> {
    let slowdown = if vm.is_bochs() { BOCHS_SLOWDOWN } else { 1 };
    let mut timeout = Duration::from_secs(10) * slowdown;
    for s in steps {
        let at = |e: String| format!("line {} `{} {}`: {e}", s.line, s.cmd, s.arg);
        // Bochs では画面を取れないので、画面を確かめる手順は飛ばす
        if vm.is_bochs()
            && matches!(s.cmd.as_str(), "screendump" | "expect-screen" | "expect-pixel" | "expect-change" | "expect-running")
        {
            continue;
        }
        match s.cmd.as_str() {
            "image" | "qemu-arg" | "writable" | "qemu-only" | "bochs-only" | "boot-config" | "net" | "net-lan" | "serve-dns" | "serve-ntp"
            | "serve-http" | "www-file" | "www-text" | "www-repeat" => {}
            "expect-packet" => vm.net.as_ref().ok_or_else(|| at("needs `net`".into()))?.expect_packet(&s.arg).map_err(at)?,
            "expect-lan-only" => vm.net.as_ref().ok_or_else(|| at("needs `net`".into()))?.expect_lan_only().map_err(at)?,
            "fake-send" => {
                vm.net.as_ref().ok_or_else(|| at("needs `net`".into()))?.send(&s.arg).map_err(at)?;
                thread::sleep(Duration::from_millis(300));
            }
            "expect-disk-same" => {
                let (path, wname) = s.arg.split_once(' ').ok_or_else(|| at("expected PATH WWWNAME".into()))?;
                let got = read_disk_file(&vm.disk, path, out).map_err(at)?;
                let want = fs::read(vm.www.join(wname.trim())).map_err(|e| at(e.to_string()))?;
                if got != want {
                    return Err(at(format!("{path} differs ({} bytes, want {})", got.len(), want.len())));
                }
            }
            "host-get" | "host-get-status" | "host-get-text" | "host-get-concurrent" => {
                let port = vm.net.as_ref().and_then(|n| n.http_port).ok_or_else(|| at("needs `net`".into()))?;
                let (path, want) = s.arg.split_once(' ').ok_or_else(|| at("expected PATH ARG".into()))?;
                let want = want.trim();
                match s.cmd.as_str() {
                    "host-get-status" => {
                        let (status, _) = net::http_get(port, path, timeout).map_err(at)?;
                        if status.to_string() != want {
                            return Err(at(format!("status {status}, want {want}")));
                        }
                    }
                    "host-get-text" => {
                        let (status, body) = net::http_get(port, path, timeout).map_err(at)?;
                        let body = String::from_utf8_lossy(&body);
                        if status != 200 || !body.contains(want) {
                            return Err(at(format!("status {status}, body does not contain the text:\n{body}")));
                        }
                    }
                    cmd => {
                        let expected = read_disk_file(&vm.disk, want, out).map_err(at)?;
                        let results = if cmd == "host-get" {
                            vec![net::http_get(port, path, timeout).map_err(at)?]
                        } else {
                            net::http_get_concurrent(port, path, timeout).map_err(at)?.to_vec()
                        };
                        for (status, body) in results {
                            if status != 200 || body != expected {
                                return Err(at(format!(
                                    "status {status}, {} bytes (want 200, {} bytes of {want})",
                                    body.len(),
                                    expected.len()
                                )));
                            }
                        }
                    }
                }
            }
            "key-hold" | "key-release" => {
                let down = s.cmd == "key-hold";
                vm.send_input(json!([{"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": s.arg}}}]))
                    .map_err(at)?;
            }
            "expect-disk-file" => {
                let (path, text) = s.arg.split_once(' ').ok_or_else(|| at("expected PATH TEXT".into()))?;
                let out = Command::new("mtype")
                    .arg("-i")
                    .arg(&vm.disk)
                    .arg(format!("::{path}"))
                    .output()
                    .map_err(|e| at(e.to_string()))?;
                let content = String::from_utf8_lossy(&out.stdout);
                if !out.status.success() || !content.contains(text) {
                    return Err(at(format!(
                        "disk file {path} does not contain the text (mtype: {}{})",
                        content.trim_end(),
                        String::from_utf8_lossy(&out.stderr).trim_end()
                    )));
                }
            }
            "expect-disk-missing" => {
                let path = s.arg.trim();
                let out = Command::new("mdir").arg("-b").arg("-i").arg(&vm.disk).arg(format!("::{path}")).output().map_err(|e| at(e.to_string()))?;
                if out.status.success() {
                    return Err(at(format!("disk file {path} exists ({})", String::from_utf8_lossy(&out.stdout).trim_end())));
                }
            }
            "quit" => {
                if let Ok(q) = vm.qmp() {
                    let _ = q.execute("quit", json!({}));
                }
                vm.wait_exit(Duration::from_secs(5)).map_err(at)?;
            }
            "expect-wav-sound" => {
                let data = fs::read(&s.arg).map_err(|e| at(format!("{}: {e}", s.arg)))?;
                // 44 バイトの見出しの後ろが 16bit の標本（符号付き）
                let samples = data.get(44..).unwrap_or(&[]);
                let loud = samples.chunks_exact(2).filter(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs() > 1000).count();
                if loud < 100 {
                    return Err(at(format!("no sound in {} ({} loud samples)", s.arg, loud)));
                }
            }
            "expect-disk-clean" => {
                let out = Command::new("fsck.fat").arg("-n").arg(&vm.disk).output().map_err(|e| at(e.to_string()))?;
                if !out.status.success() {
                    return Err(at(format!("fsck.fat found problems:\n{}", String::from_utf8_lossy(&out.stdout))));
                }
            }
            "timeout" => {
                timeout = Duration::from_secs_f32(s.arg.parse().map_err(|_| at("bad number".into()))?) * slowdown
            }
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
                // Bochs は終了コードを伝えられないので、終わったことだけを確かめる
                if got != want && !vm.is_bochs() {
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
    /// Bochs で動かしているときの画面（キー入力用）
    rfb: Option<bochs::Rfb>,
    /// 起動したイメージ（writable のときは写し）
    disk: PathBuf,
    qmp: Option<Qmp>,
    serial: Arc<Mutex<Vec<u8>>>,
    serial_pos: usize,
    sock: PathBuf,
    exit_code: Option<i32>,
    /// ネットワークのテスト（net）の偽のホスト
    net: Option<net::FakeNet>,
    /// serve-http で返すファイルの置き場所
    www: PathBuf,
}

impl Vm {
    fn start(img: &Path, extra: &[String], out: &Path, name: &str, snapshot: bool) -> Result<Vm> {
        let sock = std::env::temp_dir().join(format!("vibe-run-{}-{name}.qmp", std::process::id()));
        let _ = fs::remove_file(&sock);
        let qemu = std::env::var("QEMU").unwrap_or_else(|_| "qemu-system-i386".into());
        let mut cmd = Command::new(&qemu);
        cmd.args(["-m", "32M", "-cpu", "486"])
            .arg("-drive")
            .arg(format!("file={},format=raw,if=floppy,snapshot={}", img.display(), if snapshot { "on" } else { "off" }))
            .args(["-boot", "a", "-vga", "std", "-nic", "none", "-display", "none", "-serial", "stdio", "-monitor", "none"])
            // PC の時計はふつう現地時刻（シナリオの qemu-arg で -rtc を指定すればそちらが優先される）
            .args(["-rtc", "base=localtime"])
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

        let mut vm = Vm {
            child,
            rfb: None,
            disk: img.to_path_buf(),
            qmp: None,
            serial,
            serial_pos: 0,
            sock,
            exit_code: None,
            net: None,
            www: PathBuf::new(),
        };
        vm.qmp = Some(Qmp::connect(&vm.sock, Duration::from_secs(5))?);
        Ok(vm)
    }

    fn start_bochs(img: &Path, out: &Path, name: &str, net: bool) -> Result<Vm> {
        let serial = Arc::new(Mutex::new(Vec::new()));
        let (child, rfb) = bochs::start(img, out, name, serial.clone(), net)?;
        Ok(Vm {
            child,
            rfb: Some(rfb),
            disk: img.to_path_buf(),
            qmp: None,
            serial,
            serial_pos: 0,
            sock: PathBuf::new(),
            exit_code: None,
            net: None,
            www: PathBuf::new(),
        })
    }

    fn is_bochs(&self) -> bool {
        self.rfb.is_some()
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
        self.qmp.as_mut().ok_or_else(|| "not available on Bochs".to_string())
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
        if let Some(rfb) = self.rfb.as_mut() {
            return rfb.send_keys(keys);
        }
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

/// イメージの中のファイルを読む（mtools）
fn read_disk_file(img: &Path, path: &str, out: &Path) -> Result<Vec<u8>> {
    let tmp = out.join("disk-file.tmp");
    let _ = fs::remove_file(&tmp);
    let st = Command::new("mcopy")
        .args(["-n", "-i"])
        .arg(img)
        .arg(format!("::{path}"))
        .arg(&tmp)
        .output()
        .map_err(|e| format!("mcopy: {e}"))?;
    if !st.status.success() {
        return Err(format!("cannot read {path} from the disk: {}", String::from_utf8_lossy(&st.stderr).trim()));
    }
    let data = fs::read(&tmp).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&tmp);
    Ok(data)
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
