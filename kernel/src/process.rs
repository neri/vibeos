//! ユーザプロセス: ELF の読み込み・起動・終了
//!
//! プロセスは「ユーザ空間を持つタスク」。ELF の各 LOAD セグメントを新しいアドレス空間に置き、
//! スタックに引数と環境（argc, argv, envp）を積んで ring 3 で始める。終了コードは `wait` で受け取る。
//! fd 0〜2 は親から受け継ぐか、起動するときに指定する（`spawn_ex`）。3 以降は受け継がない。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use vibe_abi::{EXIT_INTERRUPTED, EXIT_KILLED, EXIT_PIPE, MAX_ARG_BYTES, USER_BASE, USER_END, USER_STACK_SIZE, USER_STACK_TOP, err};
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

/// 終わったプロセスの (作ったタスク, 終了コード)（wait で取り出す）
static EXIT_CODES: IrqMutex<BTreeMap<TaskId, (TaskId, i32)>> = IrqMutex::new(BTreeMap::new());

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

/// プログラムのファイルを読む。gzip（先頭が 1F 8B）なら展開する（アプリはディスクに gzip で
/// 置いている。docs/plans/app-compress.md）。圧縮していない ELF はそのまま
fn read_program(path: &str) -> Result<Vec<u8>, i32> {
    use vibe_pack::gzip::{self, GzipError};
    let data = fs::read_file(path).map_err(fs_errno)?;
    if !gzip::is_gzip(&data) {
        return Ok(data);
    }
    // 展開後の大きさの上限: ユーザ空間のプログラムを置ける範囲
    let max = (USER_STACK_TOP - USER_STACK_SIZE - USER_BASE) as usize;
    gzip::gunzip(&data, max).map_err(|e| match e {
        GzipError::OutOfMemory => err::ENOMEM,
        _ => err::ENOEXEC,
    })
}

/// ELF を読み込んでプロセスを作る。`args[0]` はプログラムの名前として渡す。環境は空、fd 0〜2 は
/// 今のタスクのものを受け継ぐ
pub fn spawn(path: &str, args: &[&str]) -> Result<TaskId, i32> {
    spawn_ex(path, args, &[], None)
}

/// 引数・環境（`名前=値`）・fd 0〜2（None なら今のタスクのものを受け継ぐ）を指定してプロセスを作る
pub fn spawn_ex(path: &str, args: &[&str], env: &[&str], stdio: Option<fs::Stdio>) -> Result<TaskId, i32> {
    let arg_bytes: usize = args.iter().chain(env).map(|a| a.len() + 1).sum();
    if arg_bytes > MAX_ARG_BYTES {
        return Err(err::E2BIG);
    }
    let data = read_program(path)?;
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
    let esp = push_args(&space, args, env).ok_or(err::ENOMEM)?;

    let name = path.rsplit('/').next().unwrap_or(path);
    let user = Arc::new(Mutex::new(UserProc { space, display: None, windows: Vec::new() }));
    Ok(sched::spawn_user(name, user, entry, esp, stdio))
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

/// スタックに引数と環境を積む。積んだ後の ESP（argc を指す）を返す
///
/// [argc][argv[0]]...[argv[n-1]][NULL][envp[0]]...[NULL] の上に文字列を置く（Unix の _start と同じ並び）
fn push_args(space: &AddressSpace, args: &[&str], env: &[&str]) -> Option<u32> {
    let mut sp = USER_STACK_TOP;
    let mut push_strs = |list: &[&str]| -> Option<Vec<u32>> {
        let mut ptrs = Vec::with_capacity(list.len());
        for a in list.iter().rev() {
            sp -= a.len() as u32 + 1;
            let mut bytes = Vec::from(a.as_bytes());
            bytes.push(0);
            if !write_user(space, sp, &bytes) {
                return None;
            }
            ptrs.push(sp);
        }
        ptrs.reverse();
        Some(ptrs)
    };
    let env_ptrs = push_strs(env)?;
    let arg_ptrs = push_strs(args)?;
    sp &= !15;
    // argc と argv[] と NULL と envp[] と NULL の分（入口で ESP が 16 の倍数になるように調整）
    let words = 1 + arg_ptrs.len() + 1 + env_ptrs.len() + 1;
    sp -= (words * 4) as u32;
    sp &= !15;
    let mut block: Vec<u8> = Vec::with_capacity(words * 4);
    block.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for p in arg_ptrs.iter().chain([&0]).chain(&env_ptrs).chain([&0]) {
        block.extend_from_slice(&p.to_le_bytes());
    }
    write_user(space, sp, &block).then_some(sp)
}

/// プロセスが終わるのを待って終了コードを返す
pub fn wait(id: TaskId) -> Result<i32, i32> {
    if !sched::exists(id) && !EXIT_CODES.lock().contains_key(&id) {
        return Err(err::ECHILD);
    }
    sched::join(id);
    Ok(EXIT_CODES.lock().remove(&id).map_or(EXIT_KILLED, |e| e.1))
}

/// wait_opt: `pid`（-1 なら今のタスクが起動したどれか）が終わっていれば (ID, 終了コード)。
/// `nohang` なら終わっていなくても待たずに None。待つものが無ければ ECHILD。止めるよう頼まれたら EINTR
pub fn wait_opt(pid: i32, nohang: bool) -> Result<Option<(TaskId, i32)>, i32> {
    let me = sched::current_id();
    let take = || {
        let mut codes = EXIT_CODES.lock();
        let id = match pid {
            -1 => codes.iter().find(|(_, e)| e.0 == me).map(|(&id, _)| id),
            p => codes.contains_key(&(p as TaskId)).then_some(p as TaskId),
        }?;
        codes.remove(&id).map(|e| (id, e.1))
    };
    let alive = || if pid == -1 { sched::has_live_child(me) } else { sched::exists(pid as TaskId) };
    loop {
        if let Some(found) = take() {
            return Ok(Some(found));
        }
        if !alive() {
            return Err(err::ECHILD);
        }
        if nohang {
            return Ok(None);
        }
        if kill_pending(me) {
            return Err(err::EINTR);
        }
        sched::wait_exit_until(|| {
            let codes = EXIT_CODES.lock();
            let done = match pid {
                -1 => codes.values().any(|e| e.0 == me),
                p => codes.contains_key(&(p as TaskId)),
            };
            drop(codes);
            done || !alive() || kill_pending(me)
        });
    }
}

/// 今のプロセスを終える。戻らないので、呼ぶ側はヒープや Arc を持ったまま呼ばないこと
pub fn exit_current(code: i32) -> ! {
    crate::drivers::speaker::off();
    let _ = catch_ctrl_c(false);
    if let Some(user) = sched::current_user() {
        close_display(&mut user.lock());
    }
    // パイプの片側などを閉じる（終了コードを待っている親より先に、読む側に終わりが届くように）
    fs::close_all();
    let me = sched::current_id();
    let parent = sched::current_parent();
    if DETACHED.lock().remove(&me).is_none() {
        EXIT_CODES.lock().insert(me, (parent, code));
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

/// 止める理由
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// kill・ウィンドウの [x]・端末を閉じた
    Kill,
    /// Ctrl+C（ctrl_c(CATCH) なら止めずに印を付ける）。true なら止めるときに "^C" を出す
    /// （パイプでつないだ集まりでは 1 つだけが出す）
    CtrlC(bool),
    /// 読む側のいないパイプに書いた
    Pipe,
}

/// 止めるよう頼まれたタスク（Ctrl+C・ウィンドウの [x] など）と理由。割り込みの中でも使うので固定長
struct KillList {
    ids: [(TaskId, Reason); 32],
    len: usize,
}

static KILL: IrqMutex<KillList> = IrqMutex::new(KillList { ids: [(0, Reason::Kill); 32], len: 0 });

/// 誰も終わるのを待たないプロセス（シェルの "&"）。終了コードを残さない
static DETACHED: IrqMutex<BTreeMap<TaskId, ()>> = IrqMutex::new(BTreeMap::new());

pub fn detach(id: TaskId) {
    // もう終わっていれば、残った終了コードを捨てる
    if EXIT_CODES.lock().remove(&id).is_none() && sched::exists(id) {
        DETACHED.lock().insert(id, ());
    }
}

/// システムコールの detach: 今のプロセスが作ったプロセスかどうかは見ない（終了コードを残さないだけ）
pub fn detach_child(id: TaskId) -> Result<(), i32> {
    if !sched::exists(id) && !EXIT_CODES.lock().contains_key(&id) {
        return Err(err::ECHILD);
    }
    detach(id);
    Ok(())
}

/// 今の端末でシェルが終わるのを待っている（端末のキーボードを使っている）プロセスの集まりを設定する
/// （空なら無し）
pub fn set_foreground(ids: &[TaskId]) {
    SCREEN.lock().set_terminal_foreground(sched::current_tty(), ids);
}

/// 端末が閉じられた: その端末につながっているプロセスをすべて止める（シェルは自分で終わる）
pub fn kill_tty(tty: WinId) {
    for t in sched::tasks() {
        if t.user.is_some() && t.tty == tty && t.state != sched::State::Dead {
            klog!("term: stopping {} ({})", t.name, t.id);
            request_kill(t.id);
        }
    }
}

/// プロセスを止めるよう頼む。ユーザモードへ戻る直前に止まる（割り込みの中から呼んでよい）
pub fn request_kill(id: TaskId) {
    request_kill_with(id, Reason::Kill);
}

/// 読む側のいないパイプに書いた: システムコールから戻るところで EXIT_PIPE で止める
pub fn request_pipe_exit(id: TaskId) {
    request_kill_with(id, Reason::Pipe);
}

fn request_kill_with(id: TaskId, reason: Reason) {
    {
        let mut k = KILL.lock();
        let len = k.len;
        if !k.ids[..len].iter().any(|e| e.0 == id) && len < k.ids.len() {
            k.ids[len] = (id, reason);
            k.len += 1;
        }
    }
    // キーやネットワークを待っているなら起こす（戻ってきたところで止める）。ほかの待ち（パイプなど）は
    // 直接起こす（WaitQueue で待っている側は起きて条件を見直し、止めるよう頼まれていることに気づく）
    gui::wake_readers();
    crate::net::wake_sockets();
    sched::wake(id);
}

/// タスクが終わるとき: 残っている依頼と Ctrl+C の印を捨てる
pub fn forget_requests(id: TaskId) {
    {
        let mut k = KILL.lock();
        while let Some(p) = k.ids[..k.len].iter().position(|e| e.0 == id) {
            let last = k.len - 1;
            k.ids[p] = k.ids[last];
            k.len -= 1;
        }
    }
    let mut c = CATCH.lock();
    if let Some(p) = c.ids[..c.len].iter().position(|e| e.0 == id) {
        let last = c.len - 1;
        c.ids[p] = c.ids[last];
        c.len -= 1;
    }
}

/// システムコールの kill: ユーザプロセスを止めるよう頼む
pub fn kill(id: TaskId) -> Result<(), i32> {
    let task = sched::tasks().into_iter().find(|t| t.id == id && t.state != sched::State::Dead);
    match task {
        None => Err(err::ECHILD),
        Some(t) if t.user.is_none() => Err(err::EPERM),
        Some(_) => {
            request_kill(id);
            Ok(())
        }
    }
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
/// その持ち主、端末ならシェルが待っているプロセスの集まりと、それらの子孫を止める
pub fn request_interrupt() {
    let targets = SCREEN.lock().interrupt_targets();
    let ids = targets.ids();
    // "^C" はユーザプロセスの最後の 1 つが出す（内蔵コマンドのスレッドはユーザモードに戻らないので出さない）
    let echo = ids.iter().rposition(|&id| sched::is_user(id));
    for (i, &id) in ids.iter().enumerate() {
        request_kill_with(id, Reason::CtrlC(Some(i) == echo));
    }
    // その子孫も止める（スクリプトの sh が起動したものなど）。裏で動かしているもの（detach）は除く
    let mut more = [0; 32];
    let n = sched::descendants(ids, &mut more, |id| DETACHED.lock().contains_key(&id));
    for &id in &more[..n] {
        request_kill_with(id, Reason::CtrlC(false));
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
    match hit {
        None => {}
        Some(Reason::CtrlC(echo)) => {
            if echo {
                kprintln!("^C");
            }
            if !mark_ctrl_c(me) {
                exit_current(EXIT_INTERRUPTED);
            }
        }
        Some(Reason::Pipe) => exit_current(EXIT_PIPE),
        Some(Reason::Kill) => exit_current(EXIT_INTERRUPTED),
    }
}
