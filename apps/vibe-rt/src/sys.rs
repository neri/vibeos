//! システムコール（int 0x80）

use core::arch::asm;

use vibe_abi::{DirEntry, MouseState, Stat, nr};

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
    syscall(nr::OPEN, path.as_ptr() as u32, path.len() as u32, 0, 0, 0)
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
