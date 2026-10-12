//! システムコール（int 0x80）

use core::arch::asm;

use vibe_abi::{DirEntry, MouseState, ProcInfo, Stat, SysInfo, nr};

/// システムコール。引数は EBX, ECX, EDX, ESI, EDI の順。
///
/// EBX と ESI は LLVM が内部で使うのでインラインアセンブラの引数にできない。番号と EBX・ESI の値は
/// メモリに置き、その番地を EAX で渡してアセンブラの中で読み込む（ほかの汎用レジスタに番地を
/// 割り当てさせると、それが ESI や EBX になったときに読み込みの途中で壊れる）
#[inline(always)]
pub fn syscall(n: u32, a: u32, b: u32, c: u32, d: u32, e: u32) -> i32 {
    let ret: i32;
    let block = [a, d, n];
    // SAFETY: カーネルへの呼び出し。引数の正しさはカーネルが確かめる
    unsafe {
        asm!(
            "push ebx",
            "push esi",
            "mov ebx, [eax]",
            "mov esi, [eax + 4]",
            "mov eax, [eax + 8]",
            "int 0x80",
            "pop esi",
            "pop ebx",
            inout("eax") block.as_ptr() => ret,
            inout("ecx") b => _,
            inout("edx") c => _,
            inout("edi") e => _,
        );
    }
    ret
}

pub fn exit(code: i32) -> ! {
    syscall(nr::EXIT, code as u32, 0, 0, 0, 0);
    unreachable!()
}

pub fn write(fd: i32, buf: &[u8]) -> i32 {
    syscall(nr::WRITE, fd as u32, buf.as_ptr() as u32, buf.len() as u32, 0, 0)
}

pub fn read(fd: i32, buf: &mut [u8]) -> i32 {
    syscall(nr::READ, fd as u32, buf.as_mut_ptr() as u32, buf.len() as u32, 0, 0)
}

pub fn open(path: &str) -> i32 {
    open_with(path, 0)
}

/// flags は vibe_abi::OPEN_*
pub fn open_with(path: &str, flags: u32) -> i32 {
    syscall(nr::OPEN, path.as_ptr() as u32, path.len() as u32, flags, 0, 0)
}

pub fn mkdir(path: &str) -> i32 {
    syscall(nr::MKDIR, path.as_ptr() as u32, path.len() as u32, 0, 0, 0)
}

pub fn remove(path: &str) -> i32 {
    syscall(nr::REMOVE, path.as_ptr() as u32, path.len() as u32, 0, 0, 0)
}

/// RAM ディスクの変更をフロッピーに書き戻す。書き戻したセクタ数
pub fn sync() -> i32 {
    syscall(nr::SYNC, 0, 0, 0, 0, 0)
}

pub fn close(fd: i32) -> i32 {
    syscall(nr::CLOSE, fd as u32, 0, 0, 0, 0)
}

pub fn sbrk(increment: i32) -> i32 {
    syscall(nr::SBRK, increment as u32, 0, 0, 0, 0)
}

pub fn sleep(ms: u32) {
    syscall(nr::SLEEP, ms, 0, 0, 0, 0);
}

pub fn ticks() -> u32 {
    syscall(nr::TICKS, 0, 0, 0, 0, 0) as u32
}

pub fn yield_now() {
    syscall(nr::YIELD, 0, 0, 0, 0, 0);
}

pub fn getpid() -> i32 {
    syscall(nr::GETPID, 0, 0, 0, 0, 0)
}

pub fn spawn(path: &str, args: &str) -> i32 {
    syscall(nr::SPAWN, path.as_ptr() as u32, path.len() as u32, args.as_ptr() as u32, args.len() as u32, 0)
}

pub fn wait(pid: i32) -> i32 {
    syscall(nr::WAIT, pid as u32, 0, 0, 0, 0)
}

pub fn stat(path: &str, st: &mut Stat) -> i32 {
    syscall(nr::STAT, path.as_ptr() as u32, path.len() as u32, st as *mut Stat as u32, 0, 0)
}

pub fn readdir(path: &str, entries: &mut [DirEntry]) -> i32 {
    syscall(nr::READDIR, path.as_ptr() as u32, path.len() as u32, entries.as_mut_ptr() as u32, entries.len() as u32, 0)
}

pub fn gfx_open(flags: u32) -> i32 {
    syscall(nr::GFX_OPEN, flags, 0, 0, 0, 0)
}

pub fn gfx_blit(buf: &[u8; 64000]) -> i32 {
    syscall(nr::GFX_BLIT, buf.as_ptr() as u32, 0, 0, 0, 0)
}

pub fn gfx_palette(start: u8, rgb: &[[u8; 3]]) -> i32 {
    syscall(nr::GFX_PALETTE, start as u32, rgb.len() as u32, rgb.as_ptr() as u32, 0, 0)
}

pub fn gfx_close() -> i32 {
    syscall(nr::GFX_CLOSE, 0, 0, 0, 0, 0)
}

/// キーイベント（nonblock で無ければ None）
pub fn getkey(nonblock: bool) -> Option<vibe_abi::KeyEvent> {
    match syscall(nr::GETKEY, nonblock as u32, 0, 0, 0, 0) {
        0 => None,
        v => Some(vibe_abi::KeyEvent(v as u32)),
    }
}

/// マウスのイベント（無ければ None。待たない）
pub fn mouse() -> Option<MouseState> {
    let mut m = MouseState::default();
    (syscall(nr::MOUSE, &mut m as *mut MouseState as u32, 0, 0, 0, 0) == 1).then_some(m)
}

/// PC スピーカーで `freq` Hz を `ms` ミリ秒鳴らす（待たない）。freq 0 で止める
pub fn sound(freq: u32, ms: u32) {
    syscall(nr::SOUND, freq, ms, 0, 0, 0);
}

/// 今の時刻（1970 年からの秒, 現地時刻）。時計が無ければ 0
pub fn time() -> u32 {
    syscall(nr::TIME, 0, 0, 0, 0, 0) as u32
}

/// ウィンドウを開く（デスクトップでは枠付き、全画面のときは前面に出る）。負ならエラー
pub fn win_open(w: u32, h: u32, title: &str) -> i32 {
    syscall(nr::WIN_OPEN, w, h, title.as_ptr() as u32, title.len() as u32, 0)
}

/// ウィンドウの中身全体を `buf`（w*h バイト）で書き換える
pub fn win_blit(win: i32, buf: &[u8]) -> i32 {
    syscall(nr::WIN_BLIT, win as u32, buf.as_ptr() as u32, 0, 0, 0)
}

/// イベントを待つ。timeout_ms = 0 なら待たない、u32::MAX ならいつまでも
pub fn win_event(win: i32, timeout_ms: u32) -> Option<vibe_abi::WinEvent> {
    let mut ev = vibe_abi::WinEvent::default();
    (syscall(nr::WIN_EVENT, win as u32, &mut ev as *mut _ as u32, timeout_ms, 0, 0) == 1).then_some(ev)
}

pub fn win_close(win: i32) -> i32 {
    syscall(nr::WIN_CLOSE, win as u32, 0, 0, 0, 0)
}

/// つながっている端末の大きさ（桁, 行）。端末が無ければ None
pub fn term_size() -> Option<(usize, usize)> {
    let v = syscall(nr::TERM_SIZE, 0, 0, 0, 0, 0);
    (v > 0).then(|| ((v as u32 >> 16) as usize, (v as u32 & 0xFFFF) as usize))
}

/// 名前を変える・移す（to が既にあるディレクトリならその中へ）
pub fn rename(from: &str, to: &str) -> i32 {
    syscall(nr::RENAME, from.as_ptr() as u32, from.len() as u32, to.as_ptr() as u32, to.len() as u32, 0)
}

/// 子の終わるのを待たない（終了コードを残さない）
pub fn detach(pid: i32) -> i32 {
    syscall(nr::DETACH, pid as u32, 0, 0, 0, 0)
}

/// 新しい端末を開く。`cmd` が空でなければそれを実行し、終わったら端末を閉じる。端末の番号
pub fn term_open(cmd: &str) -> i32 {
    syscall(nr::TERM_OPEN, cmd.as_ptr() as u32, cmd.len() as u32, 0, 0, 0)
}

/// タスクの一覧（番号の順）。書いた数
pub fn proc_list(out: &mut [ProcInfo]) -> i32 {
    syscall(nr::PROC_LIST, out.as_mut_ptr() as u32, out.len() as u32, 0, 0, 0)
}

/// プロセスを止める
pub fn kill(pid: i32) -> i32 {
    syscall(nr::KILL, pid as u32, 0, 0, 0, 0)
}

/// メモリ・時間・画面の情報
pub fn sys_info() -> SysInfo {
    let mut info = SysInfo::default();
    syscall(nr::SYS_INFO, &mut info as *mut SysInfo as u32, 0, 0, 0, 0);
    info
}

/// ウィンドウ独自のパレット（各 6bit）
pub fn win_palette(win: i32, start: u8, rgb: &[[u8; 3]]) -> i32 {
    syscall(nr::WIN_PALETTE, win as u32, start as u32, rgb.len() as u32, rgb.as_ptr() as u32, 0)
}

/// クリップボードに文字を入れる
pub fn clip_set(text: &str) -> i32 {
    syscall(nr::CLIP_SET, text.as_ptr() as u32, text.len() as u32, 0, 0, 0)
}

/// クリップボードの文字
pub fn clip_get() -> alloc::string::String {
    let mut buf = alloc::vec![0u8; 4096];
    let n = syscall(nr::CLIP_GET, buf.as_mut_ptr() as u32, buf.len() as u32, 0, 0, 0).max(0) as usize;
    buf.truncate(n.min(4096));
    alloc::string::String::from_utf8(buf).unwrap_or_default()
}
