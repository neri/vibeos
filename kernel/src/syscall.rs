//! システムコール（int 0x80）
//!
//! EAX = 番号, EBX, ECX, EDX, ESI, EDI = 引数, 戻り値は EAX（負ならエラー番号）。番号は vibe-abi。
//! トラップゲートなので割り込みは許可されたまま入ってくる（中で待ってよい）。
//!
//! ユーザから渡されたポインタは、使う前に今のプロセスのページテーブルで「ユーザが読める（書ける）
//! ページか」を確かめる。確かめた後はそのまま触る（今のアドレス空間はそのプロセスのもの）。

use alloc::string::String;
use alloc::vec::Vec;

use alloc::sync::Arc;

use vibe_abi::{
    ArpEntry, DirEntry, FsInfo, KeyEvent, MouseState, NetInfo, PciInfo, PollFd, ProcInfo, SockAddr, SockInfo, SpawnArgs,
    Stat, SysInfo, WinEvent, err, nr,
};
use vibe_net::Ipv4;

use crate::arch::trap::TrapFrame;
use crate::gui::{self, SCREEN};
use crate::drivers::pit;
use crate::fs;
use crate::io;
use crate::net;
use crate::process::{self, UserRef};
use crate::task::{self, sched};

/// アプリの画面（gfx_open）の大きさ
const APP_W: usize = 320;
const APP_H: usize = 200;

type SysResult = Result<u32, i32>;

pub fn dispatch(f: &mut TrapFrame) {
    let (n, a, b, c, d, e) = (f.eax, f.ebx, f.ecx, f.edx, f.esi, f.edi);
    // exit は戻ってこないので、プロセスへの参照（Arc）を持つ前に処理する。
    // 持ったまま終わると参照が減らず、アドレス空間が解放されない
    if n == nr::EXIT {
        process::exit_current(a as i32);
    }
    let result = match sched::current_user() {
        // ユーザプロセス以外（カーネルスレッド）からの呼び出しは受け付けない
        None => Err(err::ENOSYS),
        Some(user) => call(&user, n, a, b, c, d, e),
    };
    f.eax = match result {
        Ok(v) => v,
        Err(e) => (-e) as u32,
    };
}

fn call(user: &UserRef, n: u32, a: u32, b: u32, c: u32, d: u32, e: u32) -> SysResult {
    match n {
        nr::SOCKET..=nr::POLL | nr::ARP_LIST..=nr::SOCK_LIST => net_call(user, n, a, b, c, d, e),
        nr::CTRL_C => match a {
            vibe_abi::CTRL_C_CATCH => process::catch_ctrl_c(true).map(|_| 0),
            vibe_abi::CTRL_C_DEFAULT => process::catch_ctrl_c(false).map(|_| 0),
            vibe_abi::CTRL_C_TAKE => Ok(process::take_ctrl_c() as u32),
            _ => Err(err::EINVAL),
        },
        nr::RENAME => {
            let (from, to) = (user_str(user, a, b)?, user_str(user, c, d)?);
            fs::rename(&from, &to).map(|_| 0).map_err(process::errno_of)
        }
        nr::DETACH => process::detach_child(a).map(|_| 0),
        nr::TERM_OPEN => {
            let cmd = user_str(user, a, b)?;
            let cmd = (!cmd.trim().is_empty()).then_some(cmd);
            crate::gfx::console::open_terminal(None, cmd).ok_or(err::ENOMEM)
        }
        nr::PROC_LIST => {
            let list = sched::tasks();
            let count = (b as usize).min(list.len());
            let mut out = Vec::with_capacity(count);
            for t in &list[..count] {
                let user_pages = t.user.as_ref().map_or(0, |u| u.lock().space.pages());
                let mut p = ProcInfo {
                    id: t.id,
                    ticks: t.ticks,
                    mem_kb: ((user_pages + sched::STACK_PAGES) * 4) as u32,
                    state: match t.state {
                        sched::State::Ready => vibe_abi::PROC_READY,
                        sched::State::Running => vibe_abi::PROC_RUNNING,
                        sched::State::Blocked => vibe_abi::PROC_BLOCKED,
                        sched::State::Dead => vibe_abi::PROC_DEAD,
                    },
                    flags: if t.user.is_some() { vibe_abi::PROC_USER } else { 0 },
                    tty: t.tty,
                    ..ProcInfo::EMPTY
                };
                let name = t.name.as_bytes();
                let n = name.len().min(15);
                p.name[..n].copy_from_slice(&name[..n]);
                out.push(p);
            }
            drop(list);
            // SAFETY: ProcInfo は repr(C) で、詰め物の無い u32 と u8 だけでできている
            let bytes = unsafe {
                core::slice::from_raw_parts(out.as_ptr() as *const u8, out.len() * core::mem::size_of::<ProcInfo>())
            };
            copy_to_user(user, a, bytes)?;
            Ok(count as u32)
        }
        nr::KILL => process::kill(a).map(|_| 0),
        nr::SYS_INFO => {
            let (free, total) = crate::mm::pmm::stats();
            let (heap_free, heap_total, heap_blocks) = crate::mm::heap::stats();
            let desktop = SCREEN.lock().mode() == gui::Mode::Desktop;
            let info = SysInfo {
                heap_free: heap_free as u32,
                heap_total: heap_total as u32,
                heap_blocks: heap_blocks as u32,
                free_kb: (free * 4) as u32,
                total_kb: (total * 4) as u32,
                ticks: pit::ticks(),
                idle_ticks: sched::idle_ticks(),
                screen_width: crate::drivers::vga::width() as u32,
                screen_height: crate::drivers::vga::height() as u32,
                flags: if desktop { vibe_abi::SYS_DESKTOP } else { 0 },
            };
            copy_to_user(user, a, as_bytes(&info))?;
            Ok(0)
        }
        nr::WIN_PALETTE => {
            let id = own_window(user, a)?;
            let (start, count) = (b as usize, c as usize);
            if start + count > 256 {
                return Err(err::EINVAL);
            }
            check_user(user, d, (count * 3) as u32, false)?;
            // SAFETY: 上で確かめた
            let rgb = unsafe { core::slice::from_raw_parts(d as *const [u8; 3], count) };
            let mut s = SCREEN.lock();
            s.set_window_palette(id, start, rgb);
            s.present();
            Ok(0)
        }
        nr::CLIP_SET => {
            let text = user_str(user, a, b)?;
            crate::clipboard::set(&text);
            Ok(0)
        }
        nr::CLIP_GET => {
            let text = crate::clipboard::get();
            let n = text.len().min(b as usize);
            if n > 0 {
                copy_to_user(user, a, &text.as_bytes()[..n])?;
            }
            Ok(text.len() as u32)
        }
        nr::TERM_SIZE => {
            let (cols, rows) = crate::gfx::console::size().ok_or(err::ENOTTY)?;
            Ok((cols as u32) << 16 | rows as u32)
        }
        nr::SET_TIME => {
            crate::drivers::rtc::set_secs(a);
            Ok(0)
        }
        nr::WRITE => sys_write(user, a, b, c),
        nr::READ => sys_read(user, a, b, c),
        nr::OPEN => {
            let path = user_str(user, a, b)?;
            fs::open_with(&path, c).map(|fd| fd as u32).map_err(process::errno_of)
        }
        nr::MKDIR => fs::mkdir(&user_str(user, a, b)?).map(|_| 0).map_err(process::errno_of),
        nr::REMOVE => fs::remove(&user_str(user, a, b)?).map(|_| 0).map_err(process::errno_of),
        nr::SYNC => fs::sync().map_err(process::errno_of),
        nr::TIME => Ok(crate::drivers::rtc::now_secs()),
        nr::SOUND => {
            crate::drivers::speaker::play(a, b);
            Ok(0)
        }
        nr::CLOSE => fs::close(a as usize).map(|_| 0).map_err(process::errno_of),
        nr::PIPE => {
            check_user(user, a, 8, true)?;
            let (r, w) = io::pipe()?;
            let fds = [r as i32, w as i32];
            // SAFETY: fds は repr(C) と同じ並びの i32 の配列
            let bytes = unsafe { core::slice::from_raw_parts(fds.as_ptr() as *const u8, 8) };
            copy_to_user(user, a, bytes)?;
            Ok(0)
        }
        nr::DUP2 => fs::dup2(a as usize, b as usize).map(|fd| fd as u32).map_err(process::errno_of),
        nr::FD_KIND => fs::get(a as usize).map(|d| d.kind()).map_err(process::errno_of),
        nr::SPAWN_EX => sys_spawn_ex(user, a),
        nr::CHDIR => fs::chdir(&user_str(user, a, b)?).map(|_| 0).map_err(process::errno_of),
        nr::GETCWD => {
            let cwd = fs::cwd();
            let n = cwd.len().min(b as usize);
            if n > 0 {
                copy_to_user(user, a, &cwd.as_bytes()[..n])?;
            }
            Ok(cwd.len() as u32)
        }
        nr::WAIT_OPT => {
            if c != 0 {
                check_user(user, c, 4, true)?;
            }
            match process::wait_opt(a as i32, b & vibe_abi::WAIT_NOHANG != 0)? {
                None => Ok(0),
                Some((id, code)) => {
                    if c != 0 {
                        copy_to_user(user, c, &code.to_le_bytes())?;
                    }
                    Ok(id)
                }
            }
        }
        nr::SET_FOREGROUND => {
            if b as usize > crate::gui::MAX_FOREGROUND {
                return Err(err::EINVAL);
            }
            check_user(user, a, b * 4, false)?;
            // SAFETY: 上で今のプロセスが読めるページだと確かめた（揃っていない番地でも読めるよう read_unaligned）
            let ids: Vec<u32> = (0..b as usize).map(|i| unsafe { core::ptr::read_unaligned((a as *const u32).add(i)) }).collect();
            process::set_foreground(&ids);
            Ok(0)
        }
        nr::REBOOT => {
            klog!("reboot requested by {}", sched::current_name());
            crate::drivers::power::reboot()
        }
        nr::WALLPAPER_SIZE => {
            let s = SCREEN.lock();
            if s.mode() != gui::Mode::Desktop {
                return Err(err::EPERM);
            }
            let (w, h) = s.wallpaper_size();
            Ok((w as u32) << 16 | h as u32)
        }
        nr::WALLPAPER_SET => {
            if b == 0 {
                crate::gfx::wallpaper::clear();
                return Ok(0);
            }
            let (w, h) = {
                let s = SCREEN.lock();
                if s.mode() != gui::Mode::Desktop {
                    return Err(err::EPERM);
                }
                s.wallpaper_size()
            };
            if b as usize != w * h {
                return Err(err::EINVAL);
            }
            check_user(user, a, b, false)?;
            // SAFETY: 上で今のプロセスが読めるページだと確かめた
            let pixels = unsafe { core::slice::from_raw_parts(a as *const u8, b as usize) }.to_vec();
            let mut s = SCREEN.lock();
            s.set_wallpaper(Some(pixels.into_boxed_slice()));
            s.present();
            Ok(0)
        }
        nr::CMOS_TIME => match crate::drivers::rtc::read_cmos() {
            Ok(t) => Ok(t.to_unix() as u32),
            Err(raw) => {
                copy_to_user(user, a, &raw)?;
                Err(err::EIO)
            }
        },
        nr::DEMO => {
            if crate::demo::start() { Ok(0) } else { Err(err::EAGAIN) }
        }
        nr::FAULT_TEST => fault_test(a),
        nr::FS_INFO => {
            let (free, total) = fs::space().map_err(process::errno_of)?;
            let info = FsInfo { free_bytes: free as u32, total_bytes: total as u32, dirty_sectors: fs::dirty_sectors() };
            copy_to_user(user, a, as_bytes(&info))?;
            Ok(0)
        }
        nr::PCI_LIST => {
            use crate::drivers::pci;
            if !pci::present() {
                return Err(err::ENODEV);
            }
            let list: Vec<PciInfo> = pci::scan()
                .iter()
                .take(b as usize)
                .map(|d| PciInfo {
                    bus: d.bus,
                    dev: d.dev,
                    func: d.func,
                    class: d.class,
                    subclass: d.subclass,
                    irq: d.irq,
                    vendor: d.vendor,
                    device: d.device,
                    io: d.io_base(0).map_or(0, u32::from),
                    mem: if d.io_base(0).is_some() { 0 } else { d.mem_base(0).unwrap_or(0) },
                    ..Default::default()
                })
                .collect();
            copy_slice_to_user(user, a, &list)
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
                (de.date, de.time) = e.modified;
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
                // タイトルはプログラムの名前（拡張子を除く）
                let name = sched::current_name();
                let title = name.rsplit_once('.').map_or(name.as_str(), |(base, _)| base);
                let id = s.create_window(APP_W, APP_H, title, Some(sched::current_id()), true, Some(sched::current_tty()));
                u.display = Some(id);
            }
            s.set_cursor_visible(a & vibe_abi::GFX_CURSOR != 0);
            s.present();
            let id = u.display.unwrap();
            drop(s);
            gui::clear_mouse(id);
            Ok(0)
        }
        nr::GFX_BLIT => {
            let len = (APP_W * APP_H) as u32;
            check_user(user, a, len, false)?;
            let u = user.lock();
            let id = u.display.ok_or(err::EINVAL)?;
            // SAFETY: 上で今のプロセスが読めるページだと確かめた
            let src = unsafe { core::slice::from_raw_parts(a as *const u8, len as usize) };
            let mut s = SCREEN.lock();
            if let Some(mut surf) = s.window_surface(id) {
                surf.blit(0, 0, src, APP_W, None);
            }
            s.mark_window_dirty(id, 0, APP_H);
            s.present();
            Ok(0)
        }
        nr::GFX_PALETTE => {
            let (start, count) = (a as usize, b as usize);
            if start + count > 256 {
                return Err(err::EINVAL);
            }
            check_user(user, c, (count * 3) as u32, false)?;
            let u = user.lock();
            let id = u.display.ok_or(err::EINVAL)?;
            // SAFETY: 上で確かめた
            let rgb = unsafe { core::slice::from_raw_parts(c as *const [u8; 3], count) };
            let mut s = SCREEN.lock();
            s.set_window_palette(id, start, rgb);
            s.present();
            Ok(0)
        }
        nr::GFX_CLOSE => {
            process::close_display(&mut user.lock());
            Ok(0)
        }
        nr::WIN_OPEN => {
            let (w, h) = (a as usize, b as usize);
            if w == 0 || h == 0 || w > 640 || h > 480 {
                return Err(err::EINVAL);
            }
            let title = user_str(user, c, d)?;
            let tty = Some(sched::current_tty());
            let id = SCREEN.lock().create_window(w, h, &title, Some(sched::current_id()), false, tty);
            user.lock().windows.push(id);
            SCREEN.lock().present();
            Ok(id)
        }
        nr::WIN_BLIT => {
            let id = own_window(user, a)?;
            let (w, h) = window_size(id).ok_or(err::EINVAL)?;
            check_user(user, b, (w * h) as u32, false)?;
            // SAFETY: 上で今のプロセスが読めるページだと確かめた
            let src = unsafe { core::slice::from_raw_parts(b as *const u8, w * h) };
            let mut s = SCREEN.lock();
            if let Some(mut surf) = s.window_surface(id) {
                surf.blit(0, 0, src, w, None);
            }
            s.mark_window_dirty(id, 0, h);
            s.present();
            Ok(0)
        }
        nr::WIN_EVENT => {
            let id = own_window(user, a)?;
            check_user(user, b, core::mem::size_of::<WinEvent>() as u32, true)?;
            let timeout = match c {
                u32::MAX => None,
                ms => Some((ms * pit::TICK_HZ).div_ceil(1000)),
            };
            let ev = match gui::next_event(id, timeout) {
                None => return Ok(0),
                Some(gui::Event::Key(e)) => WinEvent {
                    kind: vibe_abi::WIN_EVENT_KEY,
                    key: KeyEvent::encode(e.ch, e.key.0, e.pressed, e.mods.shift(), e.mods.ctrl(), e.mods.alt()),
                    ..Default::default()
                },
                Some(gui::Event::Mouse(m)) => WinEvent {
                    kind: vibe_abi::WIN_EVENT_MOUSE,
                    x: m.x,
                    y: m.y,
                    buttons: m.buttons as u32,
                    ..Default::default()
                },
                Some(gui::Event::Close) => WinEvent { kind: vibe_abi::WIN_EVENT_CLOSE, ..Default::default() },
            };
            copy_to_user(user, b, as_bytes(&ev))?;
            Ok(1)
        }
        nr::WIN_CLOSE => {
            let id = own_window(user, a)?;
            user.lock().windows.retain(|&w| w != id);
            let mut s = SCREEN.lock();
            s.destroy_window(id);
            s.present();
            Ok(0)
        }
        nr::GETKEY => {
            // 画面（ウィンドウ）を開いていればそこへのキー、なければ端末へのキー
            let win = user.lock().display.unwrap_or_else(sched::current_tty);
            let ev = if a != 0 { gui::try_read_key(win) } else { gui::read_key(win) };
            Ok(ev.map_or(0, |e| {
                KeyEvent::encode(e.ch, e.key.0, e.pressed, e.mods.shift(), e.mods.ctrl(), e.mods.alt())
            }))
        }
        nr::MOUSE => {
            let Some(win) = user.lock().display else { return Ok(0) };
            match gui::poll_mouse(win) {
                Some(m) => {
                    let st = MouseState { x: m.x, y: m.y, buttons: m.buttons as u32 };
                    copy_to_user(user, a, as_bytes(&st))?;
                    Ok(1)
                }
                None => Ok(0),
            }
        }
        _ => Err(err::ENOSYS),
    }
}

fn sys_write(user: &UserRef, fd: u32, buf: u32, len: u32) -> SysResult {
    check_user(user, buf, len, false)?;
    // ファイルに書く間に待つことがあるので、いったんカーネルに写す
    // SAFETY: 上で確かめた
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len as usize) }.to_vec();
    match io::write(fd as usize, &bytes) {
        Err(err::EPIPE) => {
            // 読む側がいない: システムコールから戻るところで止める（Unix の SIGPIPE）
            process::request_pipe_exit(sched::current_id());
            Err(err::EPIPE)
        }
        r => r.map(|n| n as u32),
    }
}

fn sys_read(user: &UserRef, fd: u32, buf: u32, len: u32) -> SysResult {
    check_user(user, buf, len, true)?;
    // いったんカーネルのバッファに読んでから写す（読んでいる間に待つことがあるため）
    let mut tmp = alloc::vec![0u8; len.min(64 * 1024) as usize];
    let n = io::read(fd as usize, &mut tmp)?;
    // SAFETY: 上で確かめた
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf as *mut u8, n) };
    Ok(n as u32)
}

/// わざとカーネルの中で例外を起こす（パニック画面のシナリオ用のカーネルだけ）
#[cfg(feature = "fault-test")]
fn fault_test(kind: u32) -> SysResult {
    match kind {
        vibe_abi::FAULT_DIVZERO => {
            // Rust の割り算はゼロ除算を事前に検査して panic するので、CPU の例外を起こすには div 命令を直接使う
            // SAFETY: わざと #DE を起こす（例外ハンドラの確認用）
            unsafe { core::arch::asm!("div {0}", in(reg) 0u32, inout("eax") 1u32 => _, inout("edx") 0u32 => _) };
        }
        vibe_abi::FAULT_NULL => {
            // SAFETY: わざと 0 番地を読んでページフォルトを起こす（ページングの確認用）
            let v = unsafe { core::ptr::read_volatile(core::ptr::null::<u32>()) };
            kprintln!("read {v:#x} from address 0 (should not happen)");
        }
        vibe_abi::FAULT_STACK => {
            kprintln!("overflowing the stack...");
            recurse(0);
        }
        vibe_abi::FAULT_PANIC => panic!("panic requested by fault_test"),
        _ => return Err(err::EINVAL),
    }
    Ok(0)
}

/// スタックを使い切るまで再帰する（ガードページと二重フォルト処理の確認用）
#[cfg(feature = "fault-test")]
#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(depth: u32) -> u32 {
    let buf = [depth as u8; 512];
    let r = recurse(depth + 1);
    // 再帰の後で buf を使うので、末尾呼び出しの最適化で消えない
    core::hint::black_box(&buf)[0] as u32 + r
}

#[cfg(not(feature = "fault-test"))]
fn fault_test(_: u32) -> SysResult {
    Err(err::ENOSYS)
}

/// 0 で区切った文字列の並び（各要素の後ろに 0）を分ける
fn split_nul(text: &str) -> Vec<&str> {
    match text.strip_suffix('\0') {
        Some(t) => t.split('\0').collect(),
        None if text.is_empty() => Vec::new(),
        None => text.split('\0').collect(),
    }
}

fn sys_spawn_ex(user: &UserRef, at: u32) -> SysResult {
    check_user(user, at, core::mem::size_of::<SpawnArgs>() as u32, false)?;
    // SAFETY: 上で今のプロセスが読めるページだと確かめた
    let sa = unsafe { core::ptr::read_unaligned(at as *const SpawnArgs) };
    if sa.argv_len as usize + sa.envp_len as usize > vibe_abi::MAX_ARG_BYTES {
        return Err(err::E2BIG);
    }
    let name = user_str(user, sa.path, sa.path_len)?;
    let argv_text = user_bytes_str(user, sa.argv, sa.argv_len)?;
    let envp_text = user_bytes_str(user, sa.envp, sa.envp_len)?;
    let argv = split_nul(&argv_text);
    let envp = split_nul(&envp_text);
    if argv.is_empty() {
        return Err(err::EINVAL);
    }
    let mut stdio = fs::stdio();
    for (i, &fd) in sa.fds.iter().enumerate() {
        if fd >= 0 {
            stdio[i] = Some(fs::get(fd as usize).map_err(process::errno_of)?);
        }
    }
    let path = process::resolve_program(&name);
    process::spawn_ex(&path, &argv, &envp, Some(stdio))
}

/// 今のプロセスが win_open で作ったウィンドウか
fn own_window(user: &UserRef, id: u32) -> Result<gui::WinId, i32> {
    if user.lock().windows.contains(&id) && gui::is_owner(id, sched::current_id()) { Ok(id) } else { Err(err::EBADF) }
}

fn window_size(id: gui::WinId) -> Option<(usize, usize)> {
    let mut s = SCREEN.lock();
    let surf = s.window_surface(id)?;
    Some((surf.width(), surf.height()))
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
    user_bytes_str(user, addr, len)
}

/// ユーザの文字列を長さの上限を見ずに写す（呼ぶ側で上限を確かめる）
fn user_bytes_str(user: &UserRef, addr: u32, len: u32) -> Result<String, i32> {
    check_user(user, addr, len, false)?;
    // SAFETY: 上で確かめた
    let bytes = unsafe { core::slice::from_raw_parts(addr as *const u8, len as usize) };
    core::str::from_utf8(bytes).map(String::from).map_err(|_| err::EINVAL)
}

/// repr(C) の構造体の並びを写し、数を返す（構造体は詰め物の無いものだけにすること）
fn copy_slice_to_user<T: Copy>(user: &UserRef, addr: u32, list: &[T]) -> SysResult {
    // SAFETY: repr(C) の値の並びをバイト列として読むだけ
    let bytes = unsafe { core::slice::from_raw_parts(list.as_ptr() as *const u8, core::mem::size_of_val(list)) };
    copy_to_user(user, addr, bytes)?;
    Ok(list.len() as u32)
}

fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    // SAFETY: repr(C) の値をバイト列として読むだけ
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

// --- ネットワーク -----------------------------------------------------------------

/// その番号がソケットなら返す
fn socket_fd(fd: u32) -> Option<Arc<net::Socket>> {
    fs::socket(fd as usize)
}

fn sock_of(fd: u32) -> Result<Arc<net::Socket>, i32> {
    socket_fd(fd).ok_or(err::ENOTSOCK)
}

fn add_socket(sock: Arc<net::Socket>) -> SysResult {
    fs::add_socket(sock).map(|fd| fd as u32).map_err(process::errno_of)
}

fn read_sockaddr(user: &UserRef, addr: u32) -> Result<SockAddr, i32> {
    check_user(user, addr, core::mem::size_of::<SockAddr>() as u32, false)?;
    // SAFETY: 上で今のプロセスが読めるページだと確かめた（揃っていない番地でも読めるよう read_unaligned）
    Ok(unsafe { core::ptr::read_unaligned(addr as *const SockAddr) })
}

fn write_sockaddr(user: &UserRef, at: u32, ip: Ipv4, port: u16) -> Result<(), i32> {
    if at == 0 {
        return Ok(());
    }
    copy_to_user(user, at, as_bytes(&SockAddr::new(ip.0, port)))
}

/// 一度に poll できる数
const MAX_POLL: u32 = 64;

fn net_call(user: &UserRef, n: u32, a: u32, b: u32, c: u32, d: u32, e: u32) -> SysResult {
    match n {
        nr::SOCKET => add_socket(net::socket(a)?),
        nr::BIND => net::bind(&*sock_of(a)?, b as u16).map(|_| 0),
        nr::CONNECT => {
            let sock = sock_of(a)?;
            let to = read_sockaddr(user, b)?;
            net::connect(&sock, Ipv4(to.addr), to.port).map(|_| 0)
        }
        nr::LISTEN => net::listen(&*sock_of(a)?, b).map(|_| 0),
        nr::ACCEPT => {
            let sock = sock_of(a)?;
            if b != 0 {
                check_user(user, b, core::mem::size_of::<SockAddr>() as u32, true)?;
            }
            let (child, ip, port) = net::accept(&sock)?;
            let fd = add_socket(child)?;
            write_sockaddr(user, b, ip, port)?;
            Ok(fd)
        }
        nr::SENDTO => {
            let sock = sock_of(a)?;
            check_user(user, b, c, false)?;
            // SAFETY: 上で確かめた。送る間に待つことがあるのでカーネルに写す
            let data = unsafe { core::slice::from_raw_parts(b as *const u8, c as usize) }.to_vec();
            let (ip, port) = if d != 0 {
                let to = read_sockaddr(user, d)?;
                (Ipv4(to.addr), to.port)
            } else {
                (Ipv4::UNSPECIFIED, 0)
            };
            if sock.kind() != net::Kind::Stream && ip.is_unspecified() {
                return Err(err::EINVAL);
            }
            net::send_to(&sock, &data, ip, port).map(|n| n as u32)
        }
        nr::RECVFROM => {
            let sock = sock_of(a)?;
            check_user(user, b, c, true)?;
            if d != 0 {
                check_user(user, d, core::mem::size_of::<SockAddr>() as u32, true)?;
            }
            let mut tmp = alloc::vec![0u8; c.min(64 * 1024) as usize];
            let (n, ip, port) = net::recv_from(&sock, &mut tmp, e)?;
            // SAFETY: 上で確かめた
            unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), b as *mut u8, n) };
            write_sockaddr(user, d, ip, port)?;
            Ok(n as u32)
        }
        nr::NET_INFO => {
            let info = net::info().ok_or(err::ENODEV)?;
            let mut ni = NetInfo { mac: info.mac.0, io: info.io, irq: info.irq, ..Default::default() };
            vibe_abi::set_fixed_str(&mut ni.card, info.card);
            ni.rx_dropped = info.stats.rx_dropped;
            if let Some(c) = info.config {
                ni.flags |= vibe_abi::NET_CONFIGURED;
                ni.addr = c.addr.0;
                ni.netmask = c.netmask().0;
                ni.gateway = c.gateway.unwrap_or_default().0;
                ni.dns = c.dns.unwrap_or_default().0;
            }
            if info.dhcp {
                ni.flags |= vibe_abi::NET_DHCP;
            }
            ni.ntp = info.settings.ntp.unwrap_or_default().0;
            ni.tz_minutes = info.settings.tz_minutes;
            ni.rx_packets = info.stats.rx_frames;
            ni.tx_packets = info.stats.tx_frames;
            ni.rx_bytes = info.stats.rx_bytes;
            ni.tx_bytes = info.stats.tx_bytes;
            copy_to_user(user, a, as_bytes(&ni))?;
            Ok(0)
        }
        nr::ARP_LIST => {
            let list: Vec<ArpEntry> = net::arp_entries()
                .into_iter()
                .take(b as usize)
                .map(|(ip, mac)| ArpEntry { ip: ip.0, mac: mac.0, _pad: 0 })
                .collect();
            copy_slice_to_user(user, a, &list)
        }
        nr::ARP_RESOLVE => {
            check_user(user, c, 6, true)?;
            let mac = net::arp_resolve(Ipv4(a.to_le_bytes()), b)?;
            copy_to_user(user, c, &mac.0)?;
            Ok(0)
        }
        nr::SOCK_LIST => {
            let list: Vec<SockInfo> = net::socket_list()
                .into_iter()
                .take(b as usize)
                .map(|so| {
                    let mut si = SockInfo {
                        local_port: so.local_port,
                        recv_queued: so.recv_queued as u32,
                        send_queued: so.send_queued as u32,
                        ..Default::default()
                    };
                    vibe_abi::set_fixed_str(&mut si.kind, so.kind);
                    vibe_abi::set_fixed_str(&mut si.state, so.state);
                    if let Some((ip, port)) = so.remote {
                        si.remote = ip.0;
                        si.remote_port = port;
                    }
                    si
                })
                .collect();
            copy_slice_to_user(user, a, &list)
        }
        nr::POLL => {
            if b > MAX_POLL {
                return Err(err::EINVAL);
            }
            let size = b * core::mem::size_of::<PollFd>() as u32;
            check_user(user, a, size, true)?;
            // SAFETY: 上で確かめた
            let mut fds: Vec<PollFd> =
                (0..b as usize).map(|i| unsafe { core::ptr::read_unaligned((a as *const PollFd).add(i)) }).collect();
            let descs: Vec<Option<fs::DescRef>> =
                fds.iter().map(|p| if p.fd >= 0 { fs::get(p.fd as usize).ok() } else { None }).collect();
            // ソケットとパイプは読める・書けるが変わると net::wake_sockets で起こしてくれる
            let ready = net::wait_any(c, || {
                let mut count = 0;
                for (p, d) in fds.iter_mut().zip(&descs) {
                    let (r, w) = d.as_deref().map_or((false, false), io::readiness);
                    p.revents = (if r { vibe_abi::POLL_IN } else { 0 } | if w { vibe_abi::POLL_OUT } else { 0 }) & p.events;
                    if p.revents != 0 {
                        count += 1;
                    }
                }
                count
            })?;
            // SAFETY: 上で確かめた
            let bytes = unsafe { core::slice::from_raw_parts(fds.as_ptr() as *const u8, size as usize) };
            copy_to_user(user, a, bytes)?;
            Ok(ready as u32)
        }
        _ => Err(err::ENOSYS),
    }
}
