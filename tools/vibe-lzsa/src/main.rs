//! vibe-lzsa: LZSA の圧縮・展開と、本家の `lzsa` との比較を行うホスト側ツール
//!
//! 使い方:
//!   vibe-lzsa [-d] [-f1|-f2] [-r] [-p PARSER] [--depth N] [--arrivals K] [--extra N] IN OUT
//!       圧縮（-d で展開）。引数の形は本家の lzsa と同じ。PARSER は greedy / lazy / optimal / rep
//!       （既定は rep）。-r は raw block（64KB まで）
//!   vibe-lzsa roundtrip FILE...
//!       FILE とテスト用のデータを、すべての設定で自作の圧縮 → 自作の展開して元と比べる
//!   vibe-lzsa compat --ref LZSA [--work DIR] FILE...
//!       本家と互換かを確かめる（計画書 docs/plans/lzsa.md §5.2）。FILE とテスト用のデータを、
//!       自作で圧縮 → 本家で展開、本家で圧縮 → 自作で展開して元と比べる
//!   vibe-lzsa bench [--ref LZSA] [--work DIR] FILE...
//!       圧縮後の大きさの表（本家があれば本家との比）を出す
//!
//! 失敗があれば終了コード 1。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use vibe_lzsa::{
    BLOCK_SIZE, Options, Parser, Version, compress, decompress_raw_to_vec, decompress_to_vec,
};

const PARSERS: [(Parser, &str); 4] = [
    (Parser::Greedy, "greedy"),
    (Parser::Lazy, "lazy"),
    (Parser::Optimal, "optimal"),
    (Parser::OptimalRep, "rep"),
];

fn usage() -> ExitCode {
    eprintln!(
        "usage: vibe-lzsa [-d] [-f1|-f2] [-r] [-p PARSER] [--depth N] [--arrivals K] [--extra N] IN OUT\n\
         \x20      vibe-lzsa roundtrip FILE...\n\
         \x20      vibe-lzsa compat --ref LZSA [--work DIR] FILE...\n\
         \x20      vibe-lzsa bench [--ref LZSA] [--work DIR] FILE..."
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("roundtrip") => roundtrip(&args[1..]),
        Some("compat") => compat(&args[1..]),
        Some("bench") => bench(&args[1..]),
        Some(_) => single(&args),
        None => usage(),
    }
}

fn version_name(v: Version) -> &'static str {
    match v {
        Version::V1 => "1",
        Version::V2 => "2",
    }
}

/// 1 つのファイルを圧縮・展開する
fn single(args: &[String]) -> ExitCode {
    let mut opt = Options::new(Version::V2);
    let mut unpack = false;
    let mut files = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-d" => unpack = true,
            "-f1" => opt.version = Version::V1,
            "-f2" => opt.version = Version::V2,
            "-r" => opt.raw = true,
            "-p" => {
                let name = it.next().map(String::as_str).unwrap_or("");
                let Some(&(p, _)) = PARSERS.iter().find(|(_, n)| *n == name) else {
                    return usage();
                };
                opt.parser = p;
            }
            "--depth" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => opt.depth = n,
                None => return usage(),
            },
            "--arrivals" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) if n > 0 && n < 256 => opt.arrivals = n,
                _ => return usage(),
            },
            "--extra" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => opt.extra = n,
                None => return usage(),
            },
            _ if a.starts_with('-') => return usage(),
            _ => files.push(a.clone()),
        }
    }
    let [input, output] = &files[..] else { return usage() };
    let data = match fs::read(input) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = if unpack {
        let r = if opt.raw { decompress_raw_to_vec(opt.version, &data) } else { decompress_to_vec(&data) };
        r.map_err(|e| format!("{input}: {e:?}"))
    } else {
        compress(&data, &opt).map_err(|_| format!("{input}: does not fit in a raw block ({BLOCK_SIZE} bytes)"))
    };
    match result.and_then(|out| fs::write(output, out).map_err(|e| format!("{output}: {e}"))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// 比べる対象: 引数のファイルと（`with_samples` なら）テスト用のデータ
fn inputs(files: &[String], with_samples: bool) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut v = Vec::new();
    for f in files {
        v.push((f.clone(), fs::read(f).map_err(|e| format!("{f}: {e}"))?));
    }
    if with_samples {
        v.extend(vibe_lzsa::samples::samples().into_iter().map(|(n, d)| (format!("sample:{n}"), d)));
    }
    Ok(v)
}

/// オプションと、残りのファイル
struct Common {
    reference: Option<PathBuf>,
    work: PathBuf,
    files: Vec<String>,
}

fn common(args: &[String]) -> Option<Common> {
    let mut c = Common { reference: None, work: PathBuf::from("build/lzsa-work"), files: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ref" => c.reference = Some(PathBuf::from(it.next()?)),
            "--work" => c.work = PathBuf::from(it.next()?),
            _ if a.starts_with('-') => return None,
            _ => c.files.push(a.clone()),
        }
    }
    Some(c)
}

fn settings(len: usize) -> Vec<(Version, bool)> {
    let mut v = vec![(Version::V1, false), (Version::V2, false)];
    if len <= BLOCK_SIZE {
        v.extend([(Version::V1, true), (Version::V2, true)]);
    }
    v
}

fn setting_name(v: Version, raw: bool) -> String {
    format!("lzsa{}{}", version_name(v), if raw { "-raw" } else { "" })
}

fn unpack(v: Version, raw: bool, packed: &[u8]) -> Result<Vec<u8>, vibe_lzsa::Error> {
    if raw { decompress_raw_to_vec(v, packed) } else { decompress_to_vec(packed) }
}

fn roundtrip(args: &[String]) -> ExitCode {
    let Some(c) = common(args) else { return usage() };
    let inputs = match inputs(&c.files, true) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut checked = 0;
    let mut failed = 0;
    for (name, data) in &inputs {
        for (v, raw) in settings(data.len()) {
            for (parser, pname) in PARSERS {
                let opt = Options { raw, parser, ..Options::new(v) };
                // raw block に収まらないもの（縮まない 64KB）は飛ばす
                let Ok(packed) = compress(data, &opt) else { continue };
                checked += 1;
                if unpack(v, raw, &packed).as_deref() != Ok(&data[..]) {
                    failed += 1;
                    eprintln!("FAIL {name} {} {pname}", setting_name(v, raw));
                }
            }
        }
    }
    println!("roundtrip: {checked} checked, {failed} failed ({} inputs)", inputs.len());
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// 本家の lzsa を動かす
fn run_ref(reference: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new(reference)
        .args(args)
        .output()
        .map_err(|e| format!("{}: {e}", reference.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("lzsa {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// 本家で圧縮して中身を返す
fn ref_compress(reference: &Path, work: &Path, v: Version, raw: bool, input: &Path) -> Result<Vec<u8>, String> {
    let out = work.join("ref.lzsa");
    let f = format!("-f{}", version_name(v));
    let mut args = vec![f.as_str()];
    if raw {
        args.push("-r");
    }
    args.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
    run_ref(reference, &args)?;
    fs::read(&out).map_err(|e| e.to_string())
}

/// 本家で展開して中身を返す
fn ref_decompress(reference: &Path, work: &Path, v: Version, raw: bool, packed: &[u8]) -> Result<Vec<u8>, String> {
    let input = work.join("ours.lzsa");
    let out = work.join("ours.out");
    fs::write(&input, packed).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&out);
    let f = format!("-f{}", version_name(v));
    let mut args = vec!["-d", f.as_str()];
    if raw {
        args.push("-r");
    }
    args.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
    run_ref(reference, &args)?;
    fs::read(&out).map_err(|e| e.to_string())
}

fn compat(args: &[String]) -> ExitCode {
    let Some(c) = common(args) else { return usage() };
    let Some(reference) = c.reference.clone() else { return usage() };
    if let Err(e) = fs::create_dir_all(&c.work) {
        eprintln!("{}: {e}", c.work.display());
        return ExitCode::FAILURE;
    }
    let inputs = match inputs(&c.files, true) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let input_path = c.work.join("input.bin");
    let mut checked = 0;
    let mut failures = Vec::new();
    for (name, data) in &inputs {
        if fs::write(&input_path, data).is_err() {
            eprintln!("cannot write {}", input_path.display());
            return ExitCode::FAILURE;
        }
        for (v, raw) in settings(data.len()) {
            let set = setting_name(v, raw);
            // raw block に収まらないもの（縮まない 64KB）は本家も圧縮できない
            if compress(data, &Options { raw, ..Options::new(v) }).is_err() {
                continue;
            }
            // 自作で圧縮 → 本家で展開・自作で展開
            for (parser, pname) in PARSERS {
                let Ok(packed) = compress(data, &Options { raw, parser, ..Options::new(v) }) else {
                    failures.push(format!("{name} {set} {pname}: does not fit in a raw block"));
                    continue;
                };
                checked += 2;
                match ref_decompress(&reference, &c.work, v, raw, &packed) {
                    Ok(out) if out == *data => {}
                    Ok(_) => failures.push(format!("{name} {set} {pname}: ref lzsa -d gave different data")),
                    Err(e) => failures.push(format!("{name} {set} {pname}: {e}")),
                }
                if unpack(v, raw, &packed).as_deref() != Ok(&data[..]) {
                    failures.push(format!("{name} {set} {pname}: our decoder gave different data"));
                }
            }
            // 本家で圧縮 → 自作で展開
            checked += 1;
            match ref_compress(&reference, &c.work, v, raw, &input_path) {
                Ok(packed) => match unpack(v, raw, &packed) {
                    Ok(out) if out == *data => {}
                    Ok(_) => failures.push(format!("{name} {set} ref: our decoder gave different data")),
                    Err(e) => failures.push(format!("{name} {set} ref: our decoder: {e:?}")),
                },
                Err(e) => failures.push(format!("{name} {set} ref: {e}")),
            }
        }
    }
    for f in &failures {
        eprintln!("FAIL {f}");
    }
    println!("compat: {checked} checked, {} failed ({} inputs)", failures.len(), inputs.len());
    if failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn bench(args: &[String]) -> ExitCode {
    let Some(c) = common(args) else { return usage() };
    if c.files.is_empty() {
        return usage();
    }
    if let Err(e) = fs::create_dir_all(&c.work) {
        eprintln!("{}: {e}", c.work.display());
        return ExitCode::FAILURE;
    }
    let inputs = match inputs(&c.files, false) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let input_path = c.work.join("input.bin");
    for v in [Version::V1, Version::V2] {
        // 列: 元, 本家, 自作（区切り方ごと）
        let mut header = String::from("| file | original |");
        if c.reference.is_some() {
            header += " ref |";
        }
        for (_, n) in PARSERS {
            header += &format!(" {n} |");
        }
        println!("\nLZSA{} (stream)\n\n{header}", version_name(v));
        println!("|{}", "---|".repeat(header.matches('|').count() - 1));
        let mut total = vec![0usize; 2 + PARSERS.len()];
        let mut secs = vec![0f64; PARSERS.len()];
        for (name, data) in &inputs {
            let short = Path::new(name).file_name().map_or(name.clone(), |n| n.to_string_lossy().into_owned());
            let mut row = format!("| {short} | {} |", data.len());
            total[0] += data.len();
            if let Some(r) = &c.reference {
                if fs::write(&input_path, data).is_err() {
                    return ExitCode::FAILURE;
                }
                match ref_compress(r, &c.work, v, false, &input_path) {
                    Ok(p) => {
                        row += &format!(" {} |", p.len());
                        total[1] += p.len();
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            for (i, (parser, _)) in PARSERS.iter().enumerate() {
                let t = Instant::now();
                let n = compress(data, &Options { parser: *parser, ..Options::new(v) }).map_or(0, |p| p.len());
                secs[i] += t.elapsed().as_secs_f64();
                total[2 + i] += n;
                row += &format!(" {n} |");
            }
            println!("{row}");
        }
        let ratio = |n: usize| {
            if c.reference.is_some() { format!(" ({:.2}%)", n as f64 * 100.0 / total[1] as f64) } else { String::new() }
        };
        let mut row = format!("| **total** | {} |", total[0]);
        if c.reference.is_some() {
            row += &format!(" {} |", total[1]);
        }
        for i in 0..PARSERS.len() {
            row += &format!(" {}{} |", total[2 + i], ratio(total[2 + i]));
        }
        println!("{row}");
        let times: Vec<String> = PARSERS.iter().zip(&secs).map(|((_, n), s)| format!("{n} {s:.2}s")).collect();
        println!("\ntime: {}", times.join(", "));
    }
    ExitCode::SUCCESS
}
