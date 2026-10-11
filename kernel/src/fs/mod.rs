//! VFS（いまはルートに FAT の RAM ディスクを 1 つだけつなぐ）
//!
//! 開いているファイルの表とカレントディレクトリはタスクごと（`TaskFs`。DESIGN.md §6.4）。
//! パスは「/」区切り。相対パスはカレントディレクトリから。`.` と `..` はここで解決する。

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

use vibe_fat::{DirEntry, FatFs, FsError};

use crate::bootinfo::BootInfo;
use crate::drivers::ramdisk::RamDisk;
use crate::sync::Mutex;
use crate::task::sched;

pub type Fd = usize;
const MAX_FILES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    Fs(FsError),
    BadFd,
    TooManyFiles,
    NotMounted,
}

impl From<FsError> for VfsError {
    fn from(e: FsError) -> Self {
        VfsError::Fs(e)
    }
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VfsError::Fs(e) => write!(f, "{e}"),
            VfsError::BadFd => write!(f, "bad file descriptor"),
            VfsError::TooManyFiles => write!(f, "too many open files"),
            VfsError::NotMounted => write!(f, "no file system"),
        }
    }
}

pub type Result<T> = core::result::Result<T, VfsError>;

struct OpenFile {
    entry: DirEntry,
    pos: u32,
}

/// タスクごとのファイルシステムの状態
pub struct TaskFs {
    cwd: Vec<String>,
    files: Vec<Option<OpenFile>>,
}

impl TaskFs {
    /// 新しいタスク用（カレントディレクトリは親から受け継ぎ、開いているファイルは受け継がない）
    pub fn new_child(parent_cwd: &[String]) -> Arc<Mutex<TaskFs>> {
        Arc::new(Mutex::new(TaskFs { cwd: parent_cwd.to_vec(), files: Vec::new() }))
    }

    pub fn cwd_parts(&self) -> &[String] {
        &self.cwd
    }
}

static ROOT: Mutex<Option<FatFs<RamDisk>>> = Mutex::new(None);

/// RAM ディスクをルートにつなぐ
pub fn mount_root(info: &BootInfo) {
    // SAFETY: RAM ディスクの領域は物理ページの管理から外してあり、カーネル空間で同じ番地に対応付けられている
    let disk = unsafe { RamDisk::new(info.ramdisk_addr as usize, info.ramdisk_size as usize) };
    match FatFs::mount(disk) {
        Ok(fs) => {
            klog!(
                "fs: mounted {:?} ramdisk, {} of {} clusters free",
                fs.fat_type(),
                fs.free_clusters(),
                fs.total_clusters()
            );
            *ROOT.lock() = Some(fs);
        }
        Err(e) => klog!("fs: cannot mount ramdisk: {}", e),
    }
}

fn current() -> Arc<Mutex<TaskFs>> {
    sched::current_fs()
}

/// パスを、ルートからの名前の並びにする（`.` と `..` を解決する）
fn resolve(cwd: &[String], path: &str) -> Vec<String> {
    let mut parts: Vec<String> = if path.starts_with('/') { Vec::new() } else { cwd.to_vec() };
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name.to_ascii_uppercase()),
        }
    }
    parts
}

fn with_root<R>(f: impl FnOnce(&FatFs<RamDisk>) -> Result<R>) -> Result<R> {
    let root = ROOT.lock();
    let fs = root.as_ref().ok_or(VfsError::NotMounted)?;
    f(fs)
}

fn lookup(path: &str) -> Result<DirEntry> {
    let parts = resolve(&current().lock().cwd, path);
    with_root(|fs| Ok(fs.lookup(parts.iter().map(String::as_str))?))
}

/// ファイル・ディレクトリの情報
#[allow(dead_code)]
pub fn stat(path: &str) -> Result<DirEntry> {
    lookup(path)
}

/// ディレクトリの中身
pub fn read_dir(path: &str) -> Result<Vec<DirEntry>> {
    let entry = lookup(path)?;
    let dir = entry.as_dir()?;
    with_root(|fs| Ok(fs.read_dir(dir)?))
}

/// ファイルを読むために開く
pub fn open(path: &str) -> Result<Fd> {
    let entry = lookup(path)?;
    if entry.is_dir() {
        return Err(FsError::IsADirectory.into());
    }
    let tfs = current();
    let mut t = tfs.lock();
    let file = Some(OpenFile { entry, pos: 0 });
    if let Some(fd) = t.files.iter().position(Option::is_none) {
        t.files[fd] = file;
        return Ok(fd);
    }
    if t.files.len() >= MAX_FILES {
        return Err(VfsError::TooManyFiles);
    }
    t.files.push(file);
    Ok(t.files.len() - 1)
}

/// 読んだバイト数を返す（終わりなら 0）
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize> {
    let tfs = current();
    let mut t = tfs.lock();
    let file = t.files.get_mut(fd).and_then(Option::as_mut).ok_or(VfsError::BadFd)?;
    let n = with_root(|fs| Ok(fs.read(&file.entry, file.pos, buf)?))?;
    file.pos += n as u32;
    Ok(n)
}

pub fn close(fd: Fd) -> Result<()> {
    let tfs = current();
    let mut t = tfs.lock();
    let slot = t.files.get_mut(fd).ok_or(VfsError::BadFd)?;
    slot.take().ok_or(VfsError::BadFd)?;
    Ok(())
}

/// ファイル全体を読む
#[allow(dead_code)]
pub fn read_file(path: &str) -> Result<Vec<u8>> {
    let entry = lookup(path)?;
    with_root(|fs| Ok(fs.read_to_vec(&entry)?))
}

pub fn chdir(path: &str) -> Result<()> {
    let tfs = current();
    let mut t = tfs.lock();
    let parts = resolve(&t.cwd, path);
    let entry = with_root(|fs| Ok(fs.lookup(parts.iter().map(String::as_str))?))?;
    entry.as_dir()?;
    t.cwd = parts;
    Ok(())
}

/// カレントディレクトリ（"/" 始まり）
pub fn cwd() -> String {
    let tfs = current();
    let t = tfs.lock();
    let mut s = String::new();
    for p in &t.cwd {
        s.push('/');
        s.push_str(p);
    }
    if s.is_empty() {
        s.push('/');
    }
    s
}

/// 開いているファイルの数（テスト用）
#[allow(dead_code)]
pub fn open_count() -> usize {
    current().lock().files.iter().filter(|f| f.is_some()).count()
}
