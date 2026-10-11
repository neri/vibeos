//! システムコール（int 0x80）
//!
//! EAX = 番号, EBX, ECX, EDX, ESI, EDI = 引数, 戻り値は EAX（負ならエラー番号）。番号は vibe-abi。
//! トラップゲートなので割り込みは許可されたまま入ってくる（中で待ってよい）。
//!
//! ユーザから渡されたポインタは、使う前に今のプロセスのページテーブルで「ユーザが読める（書ける）
//! ページか」を確かめる。確かめた後はそのまま触る（今のアドレス空間はそのプロセスのもの）。

use alloc::string::String;
use alloc::vec::Vec;

use vibe_abi::{DirEntry, KeyEvent, MouseState, Stat, err, nr};

use crate::arch::trap::TrapFrame;
use crate::drivers::vga::{self, SCREEN};
use crate::drivers::{keyboard, pit};
use crate::fs;
use crate::input;
use crate::process::{self, UserRef};
use crate::shell::line::{Input, LineEditor};
use crate::task::{self, sched};

/// ファイル記述子 0〜2 はコンソール。それ以降は VFS の番号 + 3
const FIRST_FILE_FD: u32 = 3;

type SysResult = Result<u32, i32>;

pub fn dispatch(f: &mut TrapFrame) {
    let (n, a, b, c, d) = (f.eax, f.ebx, f.ecx, f.edx, f.esi);
    // exit は戻ってこないので、プロセスへの参照（Arc）を持つ前に処理する。
    // 持ったまま終わると参照が減らず、アドレス空間が解放されない
    if n == nr::EXIT {
        process::exit_current(a as i32);
    }
    let result = match sched::current_user() {
        // ユーザプロセス以外（カーネルスレッド）からの呼び出しは受け付けない
        None => Err(err::ENOSYS),
        Some(user) => call(&user, n, a, b, c, d),
    };
    f.eax = match result {
        Ok(v) => v,
        Err(e) => (-e) as u32,
    };
}

fn call(user: &UserRef, n: u32, a: u32, b: u32, c: u32, d: u32) -> SysResult {
    match n {
        nr::WRITE => sys_write(user, a, b, c),
        nr::READ => sys_read(user, a, b, c),
        nr::OPEN => {
            let path = user_str(user, a, b)?;
            fs::open_with(&path, c).map(|fd| fd as u32 + FIRST_FILE_FD).map_err(process::errno_of)
        }
        nr::MKDIR => fs::mkdir(&user_str(user, a, b)?).map(|_| 0).map_err(process::errno_of),
        nr::REMOVE => fs::remove(&user_str(user, a, b)?).map(|_| 0).map_err(process::errno_of),
        nr::SYNC => fs::sync().map_err(process::errno_of),
        nr::TIME => Ok(crate::drivers::rtc::now_secs()),
        nr::SOUND => {
            crate::drivers::speaker::play(a, b);
            Ok(0)
        }
        nr::CLOSE => {
            if a < FIRST_FILE_FD {
                return Ok(0);
            }
            fs::close((a - FIRST_FILE_FD) as usize).map(|_| 0).map_err(process::errno_of)
        }
        nr::SBRK => user.lock().space.sbrk(a as i32).ok_or(err::ENOMEM),
        nr::SLEEP => {
            pit::sleep_ms(a);
            Ok(0)
        }
        nr::TICKS => Ok(pit::ticks()),
        nr::YIELD => {
            task::yield_now();
            Ok(0)
        }
        nr::GETPID => Ok(sched::current_id()),
        nr::SPAWN => {
            let name = user_str(user, a, b)?;
            let path = process::resolve_program(&name);
            let args_text = user_str(user, c, d)?;
            let mut argv: Vec<&str> = alloc::vec![name.as_str()];
            argv.extend(args_text.split_whitespace());
            process::spawn(&path, &argv)
        }
        nr::WAIT => process::wait(a).map(|code| code as u32),
        nr::STAT => {
            let path = user_str(user, a, b)?;
            let e = fs::stat(&path).map_err(process::errno_of)?;
            let st = Stat { size: e.size, is_dir: e.is_dir() as u32 };
            copy_to_user(user, c, as_bytes(&st))?;
            Ok(0)
        }
        nr::READDIR => {
            let path = user_str(user, a, b)?;
            let entries = fs::read_dir(&path).map_err(process::errno_of)?;
            let count = (d as usize).min(entries.len());
            let mut out = Vec::with_capacity(count);
            for e in &entries[..count] {
                let mut de = DirEntry::EMPTY;
                let name = e.name.as_bytes();
                de.name[..name.len().min(15)].copy_from_slice(&name[..name.len().min(15)]);
                de.size = e.size;
                de.is_dir = e.is_dir() as u32;
                out.push(de);
            }
            // SAFETY: DirEntry は repr(C) で、詰め物の無い u8 と u32 だけでできている
            let bytes = unsafe {
                core::slice::from_raw_parts(out.as_ptr() as *const u8, out.len() * core::mem::size_of::<DirEntry>())
            };
            copy_to_user(user, c, bytes)?;
            Ok(count as u32)
        }
        nr::GFX_OPEN => {
            let mut u = user.lock();
            let mut s = SCREEN.lock();
            if u.display.is_none() {
                let id = s.create_display();
                s.set_foreground(id);
                u.display = Some(id);
            }
            s.set_cursor_visible(a & vibe_abi::GFX_CURSOR != 0);
            s.present();
            drop(s);
            input::clear_mouse_events();
            Ok(0)
        }
        nr::GFX_BLIT => {
            let len = (vga::WIDTH * vga::HEIGHT) as u32;
            check_user(user, a, len, false)?;
            let u = user.lock();
            let id = u.display.ok_or(err::EINVAL)?;
            // SAFETY: 上で今のプロセスが読めるページだと確かめた
            let src = unsafe { core::slice::from_raw_parts(a as *const u8, len as usize) };
            let mut s = SCREEN.lock();
            if let Some(mut surf) = s.display_surface(id) {
                surf.blit(0, 0, src, vga::WIDTH, None);
            }
            s.mark_display_dirty(id, 0, vga::HEIGHT);
            s.present();
            Ok(0)
        }
        nr::GFX_PALETTE => {
            let (start, count) = (a as usize, b as usize);
            if start + count > 256 {
                return Err(err::EINVAL);
            }
            check_user(user, c, (count * 3) as u32, false)?;
            let mut u = user.lock();
            if u.display.is_none() {
                return Err(err::EINVAL);
            }
            // SAFETY: 上で確かめた
            let rgb = unsafe { core::slice::from_raw_parts(c as *const [u8; 3], count) };
            vga::set_palette_range(start as u8, rgb);
            u.custom_palette = true;
            Ok(0)
        }
        nr::GFX_CLOSE => {
            process::close_display(&mut user.lock());
            Ok(0)
        }
        nr::GETKEY => {
            let ev = if a != 0 { keyboard::try_read_event() } else { Some(keyboard::read_event()) };
            Ok(ev.map_or(0, |e| {
                KeyEvent::encode(e.ch, e.key.0, e.pressed, e.mods.shift(), e.mods.ctrl(), e.mods.alt())
            }))
        }
        nr::MOUSE => match input::poll_mouse() {
            Some(m) => {
                let st = MouseState {
                    x: m.x,
                    y: m.y,
                    buttons: m.left as u32 | (m.right as u32) << 1 | (m.middle as u32) << 2,
                };
                copy_to_user(user, a, as_bytes(&st))?;
                Ok(1)
            }
            None => Ok(0),
        },
        _ => Err(err::ENOSYS),
    }
}

fn sys_write(user: &UserRef, fd: u32, buf: u32, len: u32) -> SysResult {
    check_user(user, buf, len, false)?;
    // ファイルに書く間に待つことがあるので、いったんカーネルに写す
    // SAFETY: 上で確かめた
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) }.to_vec();
    match fd {
        1 | 2 => {
            kprint!("{}", String::from_utf8_lossy(&bytes));
            Ok(len)
        }
        0 => Err(err::EBADF),
        _ => fs::write((fd - FIRST_FILE_FD) as usize, &bytes).map(|n| n as u32).map_err(process::errno_of),
    }
}

fn sys_read(user: &UserRef, fd: u32, buf: u32, len: u32) -> SysResult {
    check_user(user, buf, len, true)?;
    if fd == 0 {
        // キーボードから 1 行（編集付き）。Ctrl+C は入力の終わり（0 バイト）
        let line = match LineEditor::new().read_line("") {
            Input::Line(mut l) => {
                l.push('\n');
                l
            }
            Input::Cancelled => String::new(),
        };
        let n = line.len().min(len as usize);
        // SAFETY: 上で確かめた
        unsafe { core::ptr::copy_nonoverlapping(line.as_ptr(), buf as *mut u8, n) };
        return Ok(n as u32);
    }
    if fd < FIRST_FILE_FD {
        return Err(err::EBADF);
    }
    // ファイルの中身をいったんカーネルのバッファに読んでから写す（読んでいる間に待つことがあるため）
    let mut tmp = alloc::vec![0u8; len.min(64 * 1024) as usize];
    let n = fs::read((fd - FIRST_FILE_FD) as usize, &mut tmp).map_err(process::errno_of)?;
    // SAFETY: 上で確かめた
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n) };
    Ok(n as u32)
}

fn check_user(user: &UserRef, addr: u32, len: u32, write: bool) -> Result<(), i32> {
    if len == 0 || user.lock().space.check_user(addr, len, write) { Ok(()) } else { Err(err::EFAULT) }
}

fn copy_to_user(user: &UserRef, addr: u32, data: &[u8]) -> Result<(), i32> {
    check_user(user, addr, data.len() as u32, true)?;
    // SAFETY: 上で確かめた
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len()) };
    Ok(())
}

/// ユーザの文字列（ポインタと長さ）をカーネルに写す
fn user_str(user: &UserRef, addr: u32, len: u32) -> Result<String, i32> {
    if len > 4096 {
        return Err(err::EINVAL);
    }
    check_user(user, addr, len, false)?;
    // SAFETY: 上で確かめた
    let bytes = unsafe { core::slice::from_raw_parts(addr as *const u8, len as usize) };
    core::str::from_utf8(bytes).map(String::from).map_err(|_| err::EINVAL)
}

fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: repr(C) の値をバイト列として読むだけ
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}
