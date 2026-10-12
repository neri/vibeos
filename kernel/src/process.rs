//! ユーザプロセス: ELF の読み込み・起動・終了
//!
//! プロセスは「ユーザ空間を持つタスク」。ELF の各 LOAD セグメントを新しいアドレス空間に置き、
//! スタックに引数（argc, argv）を積んで ring 3 で始める。終了コードは `wait` で受け取る。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicU32, Ordering};

use vibe_abi::{EXIT_INTERRUPTED, EXIT_KILLED, USER_BASE, USER_END, USER_STACK_SIZE, USER_STACK_TOP, err};
use vibe_alloc::frame::PAGE_SIZE;
use xmas_elf::ElfFile;
use xmas_elf::header::{Class, Data, Machine, Type};
use xmas_elf::program::Type as PhType;

use crate::gui::{self, SCREEN, WinId};
use crate::fs;
use crate::mm::space::AddressSpace;
use crate::sync::{IrqMutex, Mutex};
use crate::task::sched::{self, TaskId};

/// プロセスが持つもの（タスクから `Arc` で参照する）
pub struct UserProc {
    pub space: AddressSpace,
    /// gfx_open で作った画面（ウィンドウ）
    pub display: Option<WinId>,
    /// win_open で作ったウィンドウ（終了時にすべて閉じる）
    pub windows: Vec<WinId>,
}

pub type UserRef = Arc<Mutex<UserProc>>;

/// 終わったプロセスの終了コード（wait で取り出す）
static EXIT_CODES: IrqMutex<BTreeMap<TaskId, i32>> = IrqMutex::new(BTreeMap::new());

fn fs_errno(e: fs::VfsError) -> i32 {
    use vibe_fat::FsError;
    match e {
        fs::VfsError::Fs(FsError::NotFound) => err::ENOENT,
        fs::VfsError::Fs(FsError::NotADirectory) => err::ENOTDIR,
        fs::VfsError::Fs(FsError::IsADirectory) => err::EISDIR,
        fs::VfsError::Fs(FsError::NoSpace) => err::ENOSPC,
        fs::VfsError::Fs(FsError::AlreadyExists) => err::EEXIST,
        fs::VfsError::Fs(FsError::InvalidName) => err::EINVAL,
        fs::VfsError::Fs(FsError::NotEmpty) => err::ENOTEMPTY,
        fs::VfsError::NotWritable => err::EBADF,
        fs::VfsError::NoDevice => err::EROFS,
        fs::VfsError::BadFd => err::EBADF,
        fs::VfsError::TooManyFiles => err::EMFILE,
        _ => err::EIO,
    }
}

pub fn errno_of(e: fs::VfsError) -> i32 {
    fs_errno(e)
}

/// ELF を読み込んでプロセスを作る。`args[0]` はプログラムの名前として渡す
pub fn spawn(path: &str, args: &[&str]) -> Result<TaskId, i32> {
    let data = fs::read_file(path).map_err(fs_errno)?;
    let elf = ElfFile::new(&data).map_err(|_| err::ENOEXEC)?;
    let h = &elf.header;
    if h.pt1.class() != Class::ThirtyTwo
        || h.pt1.data() != Data::LittleEndian
        || h.pt2.machine().as_machine() != Machine::X86
        || h.pt2.type_().as_type() != Type::Executable
    {
        return Err(err::ENOEXEC);
    }
    let entry = h.pt2.entry_point() as u32;
    if !(USER_BASE..USER_END).contains(&entry) {
        return Err(err::ENOEXEC);
    }

    let mut space = AddressSpace::new().ok_or(err::ENOMEM)?;
    let mut end = USER_BASE;
    for ph in elf.program_iter() {
        if ph.get_type() != Ok(PhType::Load) {
            continue;
        }
        let (vaddr, memsz, filesz, offset) =
            (ph.virtual_addr() as u32, ph.mem_size() as u32, ph.file_size() as u32, ph.offset() as usize);
        let seg_end = vaddr.checked_add(memsz).ok_or(err::ENOEXEC)?;
        if vaddr < USER_BASE || seg_end > USER_STACK_TOP - USER_STACK_SIZE || filesz > memsz {
            return Err(err::ENOEXEC);
        }
        let file = data.get(offset..offset + filesz as usize).ok_or(err::ENOEXEC)?;
        let writable = ph.flags().is_write();
        let mut page = vaddr & !(PAGE_SIZE as u32 - 1);
        while page < seg_end {
            space.map_user_page(page, writable).ok_or(err::ENOMEM)?;
            page += PAGE_SIZE as u32;
        }
        if !write_user(&space, vaddr, file) {
            return Err(err::ENOMEM);
        }
        end = end.max(seg_end);
    }

    // スタック（その下の 1 ページはガードとして空けておく）
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
    let mut page = stack_bottom;
    while page < USER_STACK_TOP {
        space.map_user_page(page, true).ok_or(err::ENOMEM)?;
        page += PAGE_SIZE as u32;
    }
    space.set_brk(end.next_multiple_of(PAGE_SIZE as u32), stack_bottom - PAGE_SIZE as u32);
    let esp = push_args(&space, args).ok_or(err::ENOMEM)?;

    let name = path.rsplit('/').next().unwrap_or(path);
    let user = Arc::new(Mutex::new(UserProc { space, display: None, windows: Vec::new() }));
    Ok(sched::spawn_user(name, user, entry, esp))
}

/// 別のアドレス空間のユーザ空間に書く（ページは物理アドレス経由で触る）
fn write_user(space: &AddressSpace, addr: u32, data: &[u8]) -> bool {
    let mut done = 0;
    while done < data.len() {
        let a = addr + done as u32;
        let Some((phys, _)) = crate::mm::paging::lookup(space.page_directory(), a as usize) else {
            return false;
        };
        let in_page = a as usize % PAGE_SIZE;
        let n = (PAGE_SIZE - in_page).min(data.len() - done);
        // SAFETY: phys はこのアドレス空間のページで、カーネル空間では同じ番地で触れる
        unsafe { core::ptr::copy_nonoverlapping(data[done..].as_ptr(), (phys + in_page) as *mut u8, n) };
        done += n;
    }
    true
}

/// スタックに引数を積む。積んだ後の ESP（argc を指す）を返す
///
/// [argc][argv[0]]...[argv[n-1]][NULL] の上に文字列を置く
fn push_args(space: &AddressSpace, args: &[&str]) -> Option<u32> {
    let mut sp = USER_STACK_TOP;
    let mut ptrs = Vec::with_capacity(args.len());
    for a in args.iter().rev() {
        sp -= a.len() as u32 + 1;
        let mut bytes = Vec::from(a.as_bytes());
        bytes.push(0);
        if !write_user(space, sp, &bytes) {
            return None;
        }
        ptrs.push(sp);
    }
    ptrs.reverse();
    sp &= !15;
    // argc と argv[] と NULL の分（入口で ESP が 16 の倍数になるように調整）
    let words = 1 + ptrs.len() + 1;
    sp -= (words * 4) as u32;
    sp &= !15;
    let mut block: Vec<u8> = Vec::new();
    block.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for p in &ptrs {
        block.extend_from_slice(&p.to_le_bytes());
    }
    block.extend_from_slice(&0u32.to_le_bytes());
    write_user(space, sp, &block).then_some(sp)
}

/// プロセスが終わるのを待って終了コードを返す
pub fn wait(id: TaskId) -> Result<i32, i32> {
    if !sched::exists(id) && !EXIT_CODES.lock().contains_key(&id) {
        return Err(err::ECHILD);
    }
    sched::join(id);
    Ok(EXIT_CODES.lock().remove(&id).unwrap_or(EXIT_KILLED))
}

/// 今のプロセスを終える。戻らないので、呼ぶ側はヒープや Arc を持ったまま呼ばないこと
pub fn exit_current(code: i32) -> ! {
    crate::drivers::speaker::off();
    let _ = catch_ctrl_c(false);
    if let Some(user) = sched::current_user() {
        close_display(&mut user.lock());
    }
    let me = sched::current_id();
    if DETACHED.lock().remove(&me).is_none() {
        EXIT_CODES.lock().insert(me, code);
    }
    sched::exit()
}

/// ユーザモードで例外が起きたプロセスを止める
pub fn kill_current(reason: &str, eip: u32, addr: Option<u32>) -> ! {
    {
        // exit_current は戻らないので、ヒープを使うもの（名前の String）はこの中で捨てておく
        let name = sched::current_name();
        match addr {
            Some(a) => kprintln!("\n{name}: {reason} at {eip:#010x} (address {a:#010x}), killed"),
            None => kprintln!("\n{name}: {reason} at {eip:#010x}, killed"),
        }
    }
    exit_current(EXIT_KILLED)
}

/// プロセスの画面（ウィンドウ）を閉じる。パレットは gui が前に出ているウィンドウのものに戻す
pub fn close_display(p: &mut UserProc) {
    let mut s = SCREEN.lock();
    if let Some(id) = p.display.take() {
        s.destroy_window(id);
        s.set_cursor_visible(false);
    }
    for id in p.windows.drain(..) {
        s.destroy_window(id);
    }
    s.present();
}

/// 実行ファイルの名前からパスを決める（"/" を含まなければ /BIN/名前.ELF）
pub fn resolve_program(name: &str) -> String {
    let mut path = if name.contains('/') { String::from(name) } else { alloc::format!("/BIN/{name}") };
    if !path.rsplit('/').next().unwrap_or("").contains('.') {
        path.push_str(".ELF");
    }
    path
}

const NO_PROCESS: u32 = u32::MAX;
/// シェルが終わるのを待っている（端末のキーボードを使っている）プロセス
static FOREGROUND: AtomicU32 = AtomicU32::new(NO_PROCESS);

/// 止めるよう頼まれたプロセス（Ctrl+C・ウィンドウの [x]）と、Ctrl+C によるものか。
/// 割り込みの中でも使うので固定長
struct KillList {
    ids: [(TaskId, bool); 16],
    len: usize,
}

static KILL: IrqMutex<KillList> = IrqMutex::new(KillList { ids: [(0, false); 16], len: 0 });

/// 誰も終わるのを待たないプロセス（シェルの "&"）。終了コードを残さない
static DETACHED: IrqMutex<BTreeMap<TaskId, ()>> = IrqMutex::new(BTreeMap::new());

pub fn detach(id: TaskId) {
    DETACHED.lock().insert(id, ());
}

pub fn set_foreground(id: Option<TaskId>) {
    FOREGROUND.store(id.unwrap_or(NO_PROCESS), Ordering::Relaxed);
}

/// シェルが終わるのを待っているプロセス
pub fn foreground() -> Option<TaskId> {
    let f = FOREGROUND.load(Ordering::Relaxed);
    (f != NO_PROCESS).then_some(f)
}

/// プロセスを止めるよう頼む。ユーザモードへ戻る直前に止まる（割り込みの中から呼んでよい）
pub fn request_kill(id: TaskId) {
    request_kill_with(id, false);
}

fn request_kill_with(id: TaskId, ctrl_c: bool) {
    {
        let mut k = KILL.lock();
        let len = k.len;
        if !k.ids[..len].iter().any(|e| e.0 == id) && len < k.ids.len() {
            k.ids[len] = (id, ctrl_c);
            k.len += 1;
        }
    }
    // キーやネットワークを待っているなら起こす（戻ってきたところで止める）
    gui::wake_readers();
    crate::net::wake_sockets();
}

/// 止めるよう頼まれている（待っているシステムコールは -EINTR で戻る）
pub fn kill_pending(id: TaskId) -> bool {
    let k = KILL.lock();
    k.ids[..k.len].iter().any(|e| e.0 == id)
}

/// Ctrl+C を自分で扱うプロセス（ctrl_c(CATCH)）と、押されたかの印。割り込みの中でも見るので固定長
struct CatchList {
    ids: [(TaskId, bool); 16],
    len: usize,
}

static CATCH: IrqMutex<CatchList> = IrqMutex::new(CatchList { ids: [(0, false); 16], len: 0 });

/// Ctrl+C で止めず、印を付けるだけにする（false で元に戻す）
pub fn catch_ctrl_c(on: bool) -> Result<(), i32> {
    let me = sched::current_id();
    let mut c = CATCH.lock();
    let len = c.len;
    let pos = c.ids[..len].iter().position(|e| e.0 == me);
    match (on, pos) {
        (true, None) if len < c.ids.len() => {
            c.ids[len] = (me, false);
            c.len += 1;
        }
        (true, None) => return Err(err::ENOMEM),
        (false, Some(p)) => {
            c.ids[p] = c.ids[len - 1];
            c.len -= 1;
        }
        _ => {}
    }
    Ok(())
}

/// Ctrl+C が押されたか（印を消す）
pub fn take_ctrl_c() -> bool {
    let me = sched::current_id();
    let mut c = CATCH.lock();
    let len = c.len;
    match c.ids[..len].iter_mut().find(|e| e.0 == me) {
        Some(e) => core::mem::take(&mut e.1),
        None => false,
    }
}

/// Ctrl+C を扱うプロセスなら印を付けて true
fn mark_ctrl_c(me: TaskId) -> bool {
    let mut c = CATCH.lock();
    let len = c.len;
    match c.ids[..len].iter_mut().find(|e| e.0 == me) {
        Some(e) => {
            e.1 = true;
            true
        }
        None => false,
    }
}

/// キーボードの割り込みから: Ctrl+C が押された。フォーカスのあるウィンドウがアプリのものなら
/// その持ち主、端末ならシェルが待っているプロセスを止める
pub fn request_interrupt() {
    let target = SCREEN.lock().focused_owner().or_else(foreground);
    if let Some(id) = target {
        request_kill_with(id, true);
    }
}

/// ユーザモードへ戻る直前に呼ぶ: 止めるよう頼まれていれば止める
pub fn check_interrupt() {
    let me = sched::current_id();
    let hit = {
        let mut k = KILL.lock();
        let len = k.len;
        match k.ids[..len].iter().position(|e| e.0 == me) {
            Some(p) => {
                let entry = k.ids[p];
                k.ids[p] = k.ids[len - 1];
                k.len -= 1;
                Some(entry.1)
            }
            None => None,
        }
    };
    if let Some(ctrl_c) = hit {
        if ctrl_c {
            kprintln!("^C");
            if mark_ctrl_c(me) {
                return;
            }
        }
        exit_current(EXIT_INTERRUPTED);
    }
}
