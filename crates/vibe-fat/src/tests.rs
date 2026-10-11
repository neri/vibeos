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
