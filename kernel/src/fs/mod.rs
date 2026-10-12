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
    /// 読み込み専用で開いたファイルに書こうとした
    NotWritable,
    /// 書き戻す先のフロッピードライブが無い
    NoDevice,
    /// フロッピーへの書き戻しに失敗した
    DeviceError,
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
            VfsError::NotWritable => write!(f, "file is not open for writing"),
            VfsError::NoDevice => write!(f, "no floppy drive to write to"),
            VfsError::DeviceError => write!(f, "floppy write error"),
        }
    }
}

pub type Result<T> = core::result::Result<T, VfsError>;

pub struct OpenFile {
    entry: DirEntry,
    pos: u32,
    flags: u32,
}

/// ファイルの表の 1 件（Unix の「開いたファイルの記述」）。`Arc` で持ち、複数の fd・複数のタスクで
/// 同じものを指せる（ファイルの読み書きの位置も共有する）。最後の参照が無くなると閉じる
pub enum Desc {
    /// タスクの端末（読むとキーボードから編集付きで 1 行、書くと端末とシリアル）
    Console,
    File(Mutex<OpenFile>),
    Socket(Arc<crate::net::Socket>),
    PipeRead(crate::pipe::Reader),
    PipeWrite(crate::pipe::Writer),
}

pub type DescRef = Arc<Desc>;

impl Desc {
    /// vibe_abi::FD_*
    pub fn kind(&self) -> u32 {
        match self {
            Desc::Console => vibe_abi::FD_CONSOLE,
            Desc::File(_) => vibe_abi::FD_FILE,
            Desc::Socket(_) => vibe_abi::FD_SOCKET,
            Desc::PipeRead(_) | Desc::PipeWrite(_) => vibe_abi::FD_PIPE,
        }
    }
}

pub use vibe_abi::{OPEN_APPEND, OPEN_CREATE, OPEN_TRUNCATE, OPEN_WRITE};

/// 標準入出力の 3 つ（fd 0〜2）。None は閉じている
pub type Stdio = [Option<DescRef>; 3];

/// タスクごとのファイルシステムの状態
pub struct TaskFs {
    cwd: Vec<String>,
    /// 番号がそのままファイル記述子
    files: Vec<Option<DescRef>>,
}

impl TaskFs {
    /// 最初のタスク用（fd 0〜2 は端末）
    pub fn new_root() -> Arc<Mutex<TaskFs>> {
        let console = Arc::new(Desc::Console);
        let files = alloc::vec![Some(console.clone()), Some(console.clone()), Some(console)];
        Arc::new(Mutex::new(TaskFs { cwd: Vec::new(), files }))
    }

    /// 新しいタスク用。カレントディレクトリは親から受け継ぐ。開いているファイルは fd 0〜2 だけを
    /// `stdio` のとおりにし、3 以降は受け継がない（パイプの書く側が残って終わりが来なくなるのを防ぐ）
    pub fn new_child(parent_cwd: &[String], stdio: Stdio) -> Arc<Mutex<TaskFs>> {
        Arc::new(Mutex::new(TaskFs { cwd: parent_cwd.to_vec(), files: stdio.into_iter().collect() }))
    }

    pub fn cwd_parts(&self) -> &[String] {
        &self.cwd
    }

    /// fd 0〜2 の写し（子に受け継がせる）
    pub fn stdio(&self) -> Stdio {
        core::array::from_fn(|i| self.files.get(i).cloned().flatten())
    }
}

static ROOT: Mutex<Option<FatFs<RamDisk>>> = Mutex::new(None);

/// RAM ディスクをルートにつなぐ
pub fn mount_root(info: &BootInfo) {
    // SAFETY: RAM ディスクの領域は物理ページの管理から外してあり、カーネル空間で同じ番地に対応付けられている
    let disk = unsafe { RamDisk::new(info.ramdisk_addr as usize, info.ramdisk_size as usize) };
    match FatFs::mount(disk) {
        Ok(mut fs) => {
            fs.set_clock(fat_clock);
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

/// ファイルに記録する日時（時計が読めなければ 2026-01-01）
fn fat_clock() -> (u16, u16) {
    match crate::drivers::rtc::now() {
        Some(t) => (t.fat_date(), t.fat_time()),
        None => (((2026 - 1980) << 9) | (1 << 5) | 1, 0),
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

fn with_root<R>(f: impl FnOnce(&mut FatFs<RamDisk>) -> Result<R>) -> Result<R> {
    let mut root = ROOT.lock();
    let fs = root.as_mut().ok_or(VfsError::NotMounted)?;
    f(fs)
}

/// パスを (親ディレクトリの名前の並び, 最後の名前) に分ける
fn resolve_parent(path: &str) -> Result<(Vec<String>, String)> {
    let mut parts = resolve(&current().lock().cwd, path);
    let name = parts.pop().ok_or(VfsError::Fs(FsError::InvalidName))?;
    Ok((parts, name))
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
    open_with(path, 0)
}

/// ファイルを開く。`flags` は OPEN_*（0 なら読み込み専用）
pub fn open_with(path: &str, flags: u32) -> Result<Fd> {
    install(open_desc(path, flags)?)
}

/// ファイルを開くが、表には入れない（子の fd 0〜2 にするなど）
pub fn open_desc(path: &str, flags: u32) -> Result<DescRef> {
    let entry = match lookup(path) {
        Ok(mut e) => {
            if e.is_dir() {
                return Err(FsError::IsADirectory.into());
            }
            if flags & OPEN_WRITE != 0 && flags & OPEN_TRUNCATE != 0 {
                with_root(|fs| Ok(fs.truncate(&mut e, 0)?))?;
            }
            e
        }
        Err(VfsError::Fs(FsError::NotFound)) if flags & OPEN_CREATE != 0 => {
            let (parent, name) = resolve_parent(path)?;
            with_root(|fs| {
                let dir = fs.lookup(parent.iter().map(String::as_str))?.as_dir()?;
                Ok(fs.create(dir, &name)?)
            })?
        }
        Err(e) => return Err(e),
    };
    Ok(Arc::new(Desc::File(Mutex::new(OpenFile { entry, pos: 0, flags }))))
}

/// 表の空いている一番小さい番号に入れる
pub fn install(desc: DescRef) -> Result<Fd> {
    let tfs = current();
    let mut t = tfs.lock();
    if let Some(fd) = t.files.iter().position(Option::is_none) {
        t.files[fd] = Some(desc);
        return Ok(fd);
    }
    if t.files.len() >= MAX_FILES {
        return Err(VfsError::TooManyFiles);
    }
    t.files.push(Some(desc));
    Ok(t.files.len() - 1)
}

/// ソケットを表に入れる
pub fn add_socket(sock: Arc<crate::net::Socket>) -> Result<Fd> {
    install(Arc::new(Desc::Socket(sock)))
}

/// その番号が指すもの
pub fn get(fd: Fd) -> Result<DescRef> {
    current().lock().files.get(fd).cloned().flatten().ok_or(VfsError::BadFd)
}

/// その番号がソケットなら、それを返す
pub fn socket(fd: Fd) -> Option<Arc<crate::net::Socket>> {
    match &*get(fd).ok()? {
        Desc::Socket(s) => Some(s.clone()),
        _ => None,
    }
}

/// `new` を `old` と同じものを指すようにする（`new` が開いていれば閉じる）
pub fn dup2(old: Fd, new: Fd) -> Result<Fd> {
    if new >= MAX_FILES {
        return Err(VfsError::BadFd);
    }
    let desc = get(old)?;
    let prev = {
        let tfs = current();
        let mut t = tfs.lock();
        if t.files.len() <= new {
            t.files.resize(new + 1, None);
        }
        t.files[new].replace(desc)
    };
    // 閉じるときにほかのロックを取ることがあるので、表のロックを手放してから捨てる
    drop(prev);
    Ok(new)
}

/// fd 0〜2 を入れ替えて、前のものを返す（シェルがリダイレクトの間だけつなぎ替えるのに使う）
pub fn swap_stdio(stdio: Stdio) -> Stdio {
    let tfs = current();
    let mut t = tfs.lock();
    if t.files.len() < 3 {
        t.files.resize(3, None);
    }
    let mut old: Stdio = [None, None, None];
    for (i, d) in stdio.into_iter().enumerate() {
        old[i] = core::mem::replace(&mut t.files[i], d);
    }
    old
}

/// 今のタスクの fd 0〜2 の写し
pub fn stdio() -> Stdio {
    current().lock().stdio()
}

/// 開いているものをすべて閉じる（タスクが終わるとき）
pub fn close_all() {
    let files = core::mem::take(&mut current().lock().files);
    drop(files);
}

fn with_file<R>(fd: Fd, f: impl FnOnce(&mut OpenFile) -> Result<R>) -> Result<R> {
    let desc = get(fd)?;
    match &*desc {
        Desc::File(m) => f(&mut m.lock()),
        _ => Err(VfsError::BadFd),
    }
}

/// 読んだバイト数を返す（終わりなら 0）
pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize> {
    with_file(fd, |file| read_open(file, buf))
}

/// 書いたバイト数を返す
pub fn write(fd: Fd, data: &[u8]) -> Result<usize> {
    with_file(fd, |file| write_open(file, data))
}

/// 開いたファイルから読む（位置を進める）
pub fn read_open(file: &mut OpenFile, buf: &mut [u8]) -> Result<usize> {
    let n = with_root(|fs| Ok(fs.read(&file.entry, file.pos, buf)?))?;
    file.pos += n as u32;
    Ok(n)
}

/// 開いたファイルに書く（位置を進める）
pub fn write_open(file: &mut OpenFile, data: &[u8]) -> Result<usize> {
    if file.flags & OPEN_WRITE == 0 {
        return Err(VfsError::NotWritable);
    }
    if file.flags & OPEN_APPEND != 0 {
        file.pos = file.entry.size;
    }
    let n = with_root(|fs| Ok(fs.write(&mut file.entry, file.pos, data)?))?;
    file.pos += n as u32;
    Ok(n)
}

/// ファイルを丸ごと書く（無ければ作る）
pub fn write_file(path: &str, data: &[u8]) -> Result<()> {
    let fd = open_with(path, OPEN_WRITE | OPEN_CREATE | OPEN_TRUNCATE)?;
    let r = write(fd, data);
    close(fd)?;
    r.map(|_| ())
}

pub fn mkdir(path: &str) -> Result<()> {
    let (parent, name) = resolve_parent(path)?;
    with_root(|fs| {
        let dir = fs.lookup(parent.iter().map(String::as_str))?.as_dir()?;
        fs.mkdir(dir, &name)?;
        Ok(())
    })
}

/// ファイルか空のディレクトリを消す
pub fn remove(path: &str) -> Result<()> {
    let entry = lookup(path)?;
    with_root(|fs| Ok(fs.remove(&entry)?))
}

/// 名前を変える・移す。`to` が既にあるディレクトリなら、その中へ同じ名前で移す
pub fn rename(from: &str, to: &str) -> Result<()> {
    let entry = lookup(from)?;
    let cwd = current().lock().cwd.clone();
    let to_parts = resolve(&cwd, to);
    with_root(|fs| {
        let (dir, name) = match fs.lookup(to_parts.iter().map(String::as_str)) {
            Ok(e) if e.is_dir() && e.loc != entry.loc => (e.as_dir()?, entry.name.clone()),
            _ => {
                let (name, parent) = to_parts.split_last().ok_or(VfsError::Fs(FsError::InvalidName))?;
                (fs.lookup(parent.iter().map(String::as_str))?.as_dir()?, name.clone())
            }
        };
        fs.rename(&entry, dir, &name)?;
        Ok(())
    })
}

/// RAM ディスクの変更をフロッピーに書き戻す。書いたセクタ数を返す
pub fn sync() -> Result<u32> {
    with_root(|fs| {
        let disk = fs.device_mut();
        if disk.dirty_sectors() == 0 {
            return Ok(0);
        }
        if !crate::drivers::fdc::present() {
            return Err(VfsError::NoDevice);
        }
        disk.sync().map_err(|e| {
            klog!("fs: sync failed: {:?}", e);
            VfsError::DeviceError
        })
    })
}

/// まだ書き戻していないセクタの数
pub fn dirty_sectors() -> u32 {
    with_root(|fs| Ok(fs.device().dirty_sectors())).unwrap_or(0)
}

/// (空きバイト数, 全体のバイト数)
pub fn space() -> Result<(u64, u64)> {
    with_root(|fs| Ok((fs.free_bytes(), fs.total_clusters() as u64 * fs.cluster_bytes() as u64)))
}

pub fn close(fd: Fd) -> Result<()> {
    let tfs = current();
    let open = {
        let mut t = tfs.lock();
        let slot = t.files.get_mut(fd).ok_or(VfsError::BadFd)?;
        slot.take().ok_or(VfsError::BadFd)?
    };
    // 最後の参照なら閉じる（ソケットはネットワークのロックを取るので、表のロックを手放してから捨てる）
    drop(open);
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

/// 開いているファイルの数（fd 0〜2 は数えない。テスト用）
#[allow(dead_code)]
pub fn open_count() -> usize {
    current().lock().files.iter().skip(3).filter(|f| f.is_some()).count()
}
