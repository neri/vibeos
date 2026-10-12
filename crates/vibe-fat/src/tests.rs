//! mkfs.fat と mtools で作ったイメージを読んで確かめる

use super::*;
use std::path::PathBuf;
use std::process::Command;

struct MemDisk(Vec<u8>);

impl BlockDevice for MemDisk {
    fn block_count(&self) -> u32 {
        (self.0.len() / SECTOR_SIZE) as u32
    }
    fn read_blocks(&self, lba: u32, buf: &mut [u8]) -> core::result::Result<(), IoError> {
        let start = lba as usize * SECTOR_SIZE;
        let src = self.0.get(start..start + buf.len()).ok_or(IoError)?;
        buf.copy_from_slice(src);
        Ok(())
    }
    fn write_blocks(&mut self, lba: u32, buf: &[u8]) -> core::result::Result<(), IoError> {
        let start = lba as usize * SECTOR_SIZE;
        self.0.get_mut(start..start + buf.len()).ok_or(IoError)?.copy_from_slice(buf);
        Ok(())
    }
}

fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap_or_else(|e| panic!("cannot run {cmd:?}: {e}"));
    assert!(out.status.success(), "{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// 一時ディレクトリにイメージを作る。files は (イメージ内のパス, 中身)
fn make_image(name: &str, size_kb: u32, extra_mkfs: &[&str], files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    // テストは並行に走るので、呼び出しごとに別のディレクトリを使う
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vibe-fat-test-{}-{name}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img: PathBuf = dir.join("disk.img");
    let _ = std::fs::remove_file(&img);
    let mut mkfs = Command::new("mkfs.fat");
    mkfs.arg("-C").args(extra_mkfs).arg(&img).arg(size_kb.to_string());
    run(&mut mkfs);
    for (path, data) in files {
        let parts: Vec<&str> = path.split('/').collect();
        // 途中のディレクトリを作る
        for i in 1..parts.len() {
            let d = parts[..i].join("/");
            let _ = Command::new("mmd").arg("-i").arg(&img).arg(format!("::{d}")).output();
        }
        let host = dir.join("file.tmp");
        std::fs::write(&host, data).unwrap();
        run(Command::new("mcopy").arg("-i").arg(&img).arg(&host).arg(format!("::{path}")));
    }
    let bytes = std::fs::read(&img).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

fn floppy(files: &[(&str, Vec<u8>)]) -> FatFs<MemDisk> {
    let img = make_image("floppy", 1440, &["-F", "12", "-R", "64"], files);
    FatFs::mount(MemDisk(img)).expect("mount")
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 513) as u8).collect()
}

#[test]
fn mounts_floppy_as_fat12() {
    let fs = floppy(&[]);
    assert_eq!(fs.fat_type(), FatType::Fat12);
    assert_eq!(fs.total_clusters(), 2784);
    assert_eq!(fs.free_clusters(), 2784);
    assert!(fs.read_dir(Dir::Root).unwrap().is_empty());
}

#[test]
fn lists_and_reads_root_files() {
    let fs = floppy(&[("HELLO.TXT", b"Hello, floppy!\n".to_vec()), ("NOEXT", b"x".to_vec())]);
    let names: Vec<String> = fs.read_dir(Dir::Root).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["HELLO.TXT", "NOEXT"]);
    let e = fs.lookup(["hello.txt"]).unwrap();
    assert_eq!(e.size, 15);
    assert_eq!(fs.read_to_vec(&e).unwrap(), b"Hello, floppy!\n");
}

#[test]
fn reads_multi_cluster_file_at_offsets() {
    let data = pattern(5000);
    let fs = floppy(&[("BIG.BIN", data.clone())]);
    let e = fs.lookup(["BIG.BIN"]).unwrap();
    assert_eq!(fs.read_to_vec(&e).unwrap(), data);
    let mut buf = [0u8; 700];
    assert_eq!(fs.read(&e, 1000, &mut buf).unwrap(), 700);
    assert_eq!(&buf[..], &data[1000..1700]);
    // 終わりをまたぐ・終わりから
    assert_eq!(fs.read(&e, 4800, &mut buf).unwrap(), 200);
    assert_eq!(&buf[..200], &data[4800..]);
    assert_eq!(fs.read(&e, 5000, &mut buf).unwrap(), 0);
}

#[test]
fn nested_directories() {
    let fs = floppy(&[("DOCS/README.TXT", b"read me".to_vec()), ("DOCS/SUB/DEEP.TXT", b"deep".to_vec())]);
    let docs = fs.lookup(["DOCS"]).unwrap();
    assert!(docs.is_dir());
    let names: Vec<String> = fs.read_dir(docs.as_dir().unwrap()).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["README.TXT", "SUB"]);
    let deep = fs.lookup(["docs", "sub", "deep.txt"]).unwrap();
    assert_eq!(fs.read_to_vec(&deep).unwrap(), b"deep");
}

#[test]
fn errors() {
    let fs = floppy(&[("A.TXT", b"a".to_vec()), ("D/B.TXT", b"b".to_vec())]);
    assert_eq!(fs.lookup(["NOPE"]), Err(FsError::NotFound));
    assert_eq!(fs.lookup(["A.TXT", "X"]), Err(FsError::NotADirectory));
    let d = fs.lookup(["D"]).unwrap();
    assert_eq!(fs.read(&d, 0, &mut [0u8; 4]), Err(FsError::IsADirectory));
    assert!(fs.lookup(core::iter::empty()).unwrap().is_dir());
}

#[test]
fn many_files_in_subdirectory_span_clusters() {
    let files: Vec<(String, Vec<u8>)> = (0..40).map(|i| (format!("MANY/F{i:02}.TXT"), vec![b'0' + (i % 10) as u8])).collect();
    let refs: Vec<(&str, Vec<u8>)> = files.iter().map(|(n, d)| (n.as_str(), d.clone())).collect();
    let fs = floppy(&refs);
    let dir = fs.lookup(["MANY"]).unwrap().as_dir().unwrap();
    let entries = fs.read_dir(dir).unwrap();
    assert_eq!(entries.len(), 40);
    let f37 = fs.lookup(["MANY", "F37.TXT"]).unwrap();
    assert_eq!(fs.read_to_vec(&f37).unwrap(), b"7");
}

#[test]
fn mounts_fat16() {
    let data = pattern(20000);
    let img = make_image("fat16", 16 * 1024, &["-F", "16"], &[("X/DATA.BIN", data.clone())]);
    let fs = FatFs::mount(MemDisk(img)).unwrap();
    assert_eq!(fs.fat_type(), FatType::Fat16);
    let e = fs.lookup(["X", "DATA.BIN"]).unwrap();
    assert_eq!(fs.read_to_vec(&e).unwrap(), data);
}

#[test]
fn rejects_garbage() {
    assert!(matches!(FatFs::mount(MemDisk(vec![0u8; 1440 * 1024])), Err(FsError::BadFormat(_))));
}

// --- 書き込み ---------------------------------------------------------------

/// イメージを一時ファイルに書き出して fsck.fat で検査し、mtype でファイルの中身を読む
fn host_check(fs: &FatFs<MemDisk>, files: &[(&str, &[u8])]) {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vibe-fat-check-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("check.img");
    std::fs::write(&img, &fs.device().0).unwrap();
    let out = Command::new("fsck.fat").arg("-n").arg(&img).output().unwrap();
    assert!(out.status.success(), "fsck.fat found problems:\n{}", String::from_utf8_lossy(&out.stdout));
    for (path, want) in files {
        let out = Command::new("mtype").arg("-i").arg(&img).arg(format!("::{path}")).output().unwrap();
        assert!(out.status.success(), "mtype {path}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(&out.stdout, want, "content of {path} as seen by mtools");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn create_and_write_small_file() {
    let mut fs = floppy(&[]);
    let mut f = fs.create(Dir::Root, "note.txt").unwrap();
    assert_eq!(f.name, "NOTE.TXT");
    fs.write(&mut f, 0, b"hello").unwrap();
    fs.write(&mut f, 5, b", world\n").unwrap();
    let e = fs.lookup(["NOTE.TXT"]).unwrap();
    assert_eq!(fs.read_to_vec(&e).unwrap(), b"hello, world\n");
    host_check(&fs, &[("NOTE.TXT", b"hello, world\n")]);
}

#[test]
fn write_multi_cluster_overwrite_and_gap() {
    let mut fs = floppy(&[]);
    let free_before = fs.free_clusters();
    let mut f = fs.create(Dir::Root, "BIG.BIN").unwrap();
    let mut data = pattern(3000);
    fs.write(&mut f, 0, &data).unwrap();
    assert_eq!(fs.free_clusters(), free_before - 6);
    // 途中を上書き（クラスタの境界をまたぐ）
    fs.write(&mut f, 1000, &[0xAA; 100]).unwrap();
    data[1000..1100].fill(0xAA);
    // 終わりより後ろに書くと間は 0
    fs.write(&mut f, 4000, b"END").unwrap();
    data.resize(4000, 0);
    data.extend_from_slice(b"END");
    assert_eq!(fs.read_to_vec(&fs.lookup(["BIG.BIN"]).unwrap()).unwrap(), data);
    host_check(&fs, &[("BIG.BIN", &data)]);
}

#[test]
fn truncate_and_remove_free_clusters() {
    let mut fs = floppy(&[("OLD.TXT", pattern(5000))]);
    let free0 = fs.free_clusters();
    let mut f = fs.lookup(["OLD.TXT"]).unwrap();
    fs.truncate(&mut f, 600).unwrap();
    assert_eq!(fs.free_clusters(), free0 + 8);
    assert_eq!(fs.read_to_vec(&fs.lookup(["OLD.TXT"]).unwrap()).unwrap(), &pattern(5000)[..600]);
    host_check(&fs, &[("OLD.TXT", &pattern(5000)[..600])]);
    fs.truncate(&mut f, 0).unwrap();
    assert_eq!(fs.free_clusters(), free0 + 10);
    let f = fs.lookup(["OLD.TXT"]).unwrap();
    fs.remove(&f).unwrap();
    assert_eq!(fs.lookup(["OLD.TXT"]), Err(FsError::NotFound));
    host_check(&fs, &[]);
}

#[test]
fn mkdir_nested_files_and_growing_directory() {
    let mut fs = floppy(&[]);
    let d = fs.mkdir(Dir::Root, "work").unwrap();
    let sub = fs.mkdir(d.as_dir().unwrap(), "sub").unwrap();
    let mut f = fs.create(sub.as_dir().unwrap(), "deep.txt").unwrap();
    fs.write(&mut f, 0, b"deep").unwrap();
    // 1 クラスタ（16 エントリ）を超えるだけファイルを作るとディレクトリが伸びる
    let wd = d.as_dir().unwrap();
    for i in 0..40 {
        let mut f = fs.create(wd, &format!("F{i}.TXT")).unwrap();
        fs.write(&mut f, 0, format!("{i}").as_bytes()).unwrap();
    }
    assert_eq!(fs.read_dir(wd).unwrap().len(), 41);
    assert_eq!(fs.read_to_vec(&fs.lookup(["WORK", "F39.TXT"]).unwrap()).unwrap(), b"39");
    host_check(&fs, &[("WORK/SUB/DEEP.TXT", b"deep"), ("WORK/F39.TXT", b"39"), ("WORK/F0.TXT", b"0")]);
    // 空でないディレクトリは消せない
    assert_eq!(fs.remove(&fs.lookup(["WORK"]).unwrap()), Err(FsError::NotEmpty));
    let f = fs.lookup(["WORK", "SUB", "DEEP.TXT"]).unwrap();
    fs.remove(&f).unwrap();
    let s = fs.lookup(["WORK", "SUB"]).unwrap();
    fs.remove(&s).unwrap();
    host_check(&fs, &[]);
}

#[test]
fn name_rules_and_duplicates() {
    let mut fs = floppy(&[("A.TXT", b"a".to_vec())]);
    assert_eq!(fs.create(Dir::Root, "a.txt").err(), Some(FsError::AlreadyExists));
    for bad in ["toolongname.txt", "x.long", "", ".txt", "sp ace", "a/b", "dot.."] {
        assert_eq!(fs.create(Dir::Root, bad).err(), Some(FsError::InvalidName), "{bad:?}");
    }
    assert!(fs.create(Dir::Root, "NOEXT").is_ok());
}

#[test]
fn root_directory_and_disk_can_fill_up() {
    let mut fs = floppy(&[]);
    // ルートディレクトリは 224 エントリまで
    for i in 0..224 {
        fs.create(Dir::Root, &format!("R{i}")).unwrap();
    }
    assert_eq!(fs.create(Dir::Root, "ONEMORE").err(), Some(FsError::NoSpace));
    // 入りきらない書き込みは NoSpace で、取りかけたクラスタは返す（ファイルは元のまま）
    let mut f = fs.lookup(["R0"]).unwrap();
    let big = vec![1u8; 1024 * 1024];
    fs.write(&mut f, 0, &big).unwrap();
    let free = fs.free_clusters();
    assert_eq!(fs.write(&mut f, 1024 * 1024, &big).err(), Some(FsError::NoSpace));
    assert_eq!(fs.free_clusters(), free);
    assert_eq!(fs.lookup(["R0"]).unwrap().size, 1024 * 1024);
    host_check(&fs, &[]);
}

#[test]
fn fat16_write() {
    let img = make_image("fat16w", 16 * 1024, &["-F", "16"], &[]);
    let mut fs = FatFs::mount(MemDisk(img)).unwrap();
    let d = fs.mkdir(Dir::Root, "DATA").unwrap();
    let mut f = fs.create(d.as_dir().unwrap(), "X.BIN").unwrap();
    let data = pattern(70000);
    fs.write(&mut f, 0, &data).unwrap();
    host_check(&fs, &[("DATA/X.BIN", &data)]);
}

#[test]
fn timestamps_come_from_the_clock() {
    fn clock() -> (u16, u16) {
        // 2026-10-11 21:15:36
        (((2026 - 1980) << 9) | (10 << 5) | 11, (21 << 11) | (15 << 5) | 18)
    }
    let mut fs = floppy(&[]);
    fs.set_clock(clock);
    let mut f = fs.create(Dir::Root, "TIME.TXT").unwrap();
    fs.write(&mut f, 0, b"t").unwrap();
    assert_eq!(fs.lookup(["TIME.TXT"]).unwrap().modified, clock());
    // mtools でも同じ日時に見える
    let dir = std::env::temp_dir().join(format!("vibe-fat-time-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("t.img");
    std::fs::write(&img, &fs.device().0).unwrap();
    let out = Command::new("mdir").arg("-i").arg(&img).arg("::TIME.TXT").output().unwrap();
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(listing.contains("2026-10-11") && listing.contains("21:15"), "{listing}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rename_in_place_and_move_between_directories() {
    let mut fs = floppy(&[("A.TXT", b"apple".to_vec()), ("B.TXT", b"banana".to_vec())]);
    let docs = fs.mkdir(Dir::Root, "DOCS").unwrap();
    let sub = fs.mkdir(docs.as_dir().unwrap(), "SUB").unwrap();
    // 同じディレクトリの中で名前を変える
    let a = fs.lookup(["A.TXT"]).unwrap();
    let renamed = fs.rename(&a, Dir::Root, "apricot.txt").unwrap();
    assert_eq!(renamed.name, "APRICOT.TXT");
    assert_eq!(renamed.loc, a.loc, "same slot");
    assert_eq!(fs.lookup(["A.TXT"]), Err(FsError::NotFound));
    // 別のディレクトリへ移す
    let b = fs.lookup(["B.TXT"]).unwrap();
    fs.rename(&b, sub.as_dir().unwrap(), "B.TXT").unwrap();
    assert_eq!(fs.lookup(["B.TXT"]), Err(FsError::NotFound));
    assert_eq!(fs.read_to_vec(&fs.lookup(["DOCS", "SUB", "B.TXT"]).unwrap()).unwrap(), b"banana");
    // 同じ名前があれば失敗し、何も変わらない
    let ap = fs.lookup(["APRICOT.TXT"]).unwrap();
    let mut f = fs.create(docs.as_dir().unwrap(), "APRICOT.TXT").unwrap();
    fs.write(&mut f, 0, b"x").unwrap();
    assert_eq!(fs.rename(&ap, docs.as_dir().unwrap(), "APRICOT.TXT").err(), Some(FsError::AlreadyExists));
    assert_eq!(fs.rename(&ap, Dir::Root, "bad name").err(), Some(FsError::InvalidName));
    // 同じ名前への変更は何もしない
    assert!(fs.rename(&ap, Dir::Root, "apricot.TXT").is_ok());
    host_check(&fs, &[("APRICOT.TXT", b"apple"), ("DOCS/SUB/B.TXT", b"banana"), ("DOCS/APRICOT.TXT", b"x")]);
}

#[test]
fn moving_a_directory_fixes_dotdot_and_refuses_cycles() {
    let mut fs = floppy(&[]);
    let a = fs.mkdir(Dir::Root, "A").unwrap();
    let b = fs.mkdir(a.as_dir().unwrap(), "B").unwrap();
    let mut f = fs.create(b.as_dir().unwrap(), "F.TXT").unwrap();
    fs.write(&mut f, 0, b"in b").unwrap();
    // 自分の中・子孫の中へは移せない
    assert_eq!(fs.rename(&a, b.as_dir().unwrap(), "A").err(), Some(FsError::InvalidName));
    assert_eq!(fs.rename(&a, a.as_dir().unwrap(), "A2").err(), Some(FsError::InvalidName));
    // B をルートへ移すと、B の ".." はルート（0）を指す（fsck.fat が確かめる）
    let b = fs.lookup(["A", "B"]).unwrap();
    fs.rename(&b, Dir::Root, "B").unwrap();
    assert_eq!(fs.read_to_vec(&fs.lookup(["B", "F.TXT"]).unwrap()).unwrap(), b"in b");
    assert!(fs.read_dir(a.as_dir().unwrap()).unwrap().is_empty());
    host_check(&fs, &[("B/F.TXT", b"in b")]);
    // ルートからサブディレクトリへ戻す
    let b = fs.lookup(["B"]).unwrap();
    fs.rename(&b, a.as_dir().unwrap(), "C").unwrap();
    host_check(&fs, &[("A/C/F.TXT", b"in b")]);
}
