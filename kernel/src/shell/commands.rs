//! シェルのコマンド

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

use vibe_font::FONT_8X8;
use vibe_gfx::palette;

use super::Shell;
use crate::gui::SCREEN;
use crate::drivers::{pit, power};
use crate::fs;
use crate::mm::{heap, pmm};
use crate::task::{self, sched};

pub struct Command {
    pub name: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    pub run: fn(&mut Shell, &[&str]),
}

const COMMANDS: &[Command] = &[
    Command { name: "help", usage: "help [command]", help: "show commands", run: help },
    Command { name: "clear", usage: "clear", help: "clear the screen", run: clear },
    Command { name: "echo", usage: "echo [text...]", help: "print text", run: echo },
    Command { name: "ls", usage: "ls [dir]", help: "list a directory", run: ls },
    Command { name: "cat", usage: "cat file...", help: "print files", run: cat },
    Command { name: "hexdump", usage: "hexdump file", help: "dump a file in hex", run: hexdump },
    Command { name: "write", usage: "write file text...", help: "write text to a file (replaces it)", run: write },
    Command { name: "append", usage: "append file text...", help: "add a line of text to a file", run: append },
    Command { name: "cp", usage: "cp from to", help: "copy a file", run: cp },
    Command { name: "rm", usage: "rm path", help: "remove a file or an empty directory", run: rm },
    Command { name: "mkdir", usage: "mkdir dir", help: "make a directory", run: mkdir },
    Command { name: "df", usage: "df", help: "disk space and unsaved changes", run: df },
    Command { name: "sync", usage: "sync", help: "save changes to the floppy disk", run: sync },
    Command { name: "cd", usage: "cd [dir]", help: "change directory", run: cd },
    Command { name: "pwd", usage: "pwd", help: "print current directory", run: pwd },
    Command { name: "mem", usage: "mem", help: "memory usage", run: mem },
    Command { name: "ps", usage: "ps", help: "list tasks", run: ps },
    Command { name: "uptime", usage: "uptime", help: "time since boot", run: uptime },
    Command { name: "date", usage: "date", help: "current date and time", run: date },
    Command { name: "hwclock", usage: "hwclock", help: "read the CMOS clock", run: hwclock },
    Command { name: "sleep", usage: "sleep [ms]", help: "sleep (default 500ms, checks timing)", run: sleep },
    Command { name: "history", usage: "history", help: "command history", run: history },
    Command { name: "term", usage: "term", help: "open a new terminal (Alt+F1-F4 to switch)", run: term },
    Command { name: "demo", usage: "demo", help: "3 threads draw counters for 5s", run: demo },
    Command { name: "beep", usage: "beep [hz] [ms]", help: "sound the PC speaker", run: beep },
    Command { name: "net", usage: "net", help: "network card, address, sockets", run: net_cmd },
    Command { name: "arp", usage: "arp [ip]", help: "find the MAC address of an IP (or list)", run: arp },
    Command { name: "pci", usage: "pci", help: "list PCI devices", run: pci },
    Command { name: "reboot", usage: "reboot", help: "restart the computer", run: reboot },
    Command { name: "divzero", usage: "divzero", help: "test: divide by zero", run: divzero },
    Command { name: "nullptr", usage: "nullptr", help: "test: read address 0", run: nullptr },
    Command { name: "overflow", usage: "overflow", help: "test: kernel stack overflow", run: overflow },
    Command { name: "panic", usage: "panic", help: "test: kernel panic", run: panic_cmd },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

fn help(_: &mut Shell, args: &[&str]) {
    if let Some(name) = args.get(1) {
        match find(name) {
            Some(c) => kprintln!("{}\n  {}", c.usage, c.help),
            None => kprintln!("help: no such command: {name}"),
        }
        return;
    }
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    // 端末の幅に収まるように並べる
    let width = crate::gfx::console::size().map_or(40, |(cols, _)| cols);
    let mut line = String::new();
    for n in names {
        if line.len() + n.len() + 1 > width - 2 {
            kprintln!("{line}");
            line.clear();
        }
        line.push_str(n);
        line.push(' ');
    }
    kprintln!("{line}");
    kprintln!("'help <command>' for details");
}

fn clear(_: &mut Shell, _: &[&str]) {
    kprint!("\x1b[2J\x1b[H");
}

fn echo(_: &mut Shell, args: &[&str]) {
    kprintln!("{}", args[1..].join(" "));
}

fn ls(_: &mut Shell, args: &[&str]) {
    let path = args.get(1).copied().unwrap_or(".");
    match fs::read_dir(path) {
        Ok(entries) => {
            for e in entries {
                let when = vibe_time::DateTime::from_fat(e.modified.0, e.modified.1)
                    .map(|t| format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute))
                    .unwrap_or_default();
                if e.is_dir() {
                    kprintln!(" \x1b[94m{:12}\x1b[0m   <DIR> {when}", e.name);
                } else {
                    kprintln!(" {:12} {:7} {when}", e.name, e.size);
                }
            }
        }
        Err(e) => kprintln!("ls: {path}: {e}"),
    }
}

fn cat(_: &mut Shell, args: &[&str]) {
    if args.len() < 2 {
        kprintln!("usage: cat file...");
    }
    for path in &args[1..] {
        if let Err(e) = cat_file(path) {
            kprintln!("cat: {path}: {e}");
        }
    }
}

/// ファイルを少しずつ読んで表示する（表示できない文字は '.'）
fn cat_file(path: &str) -> fs::Result<()> {
    let fd = fs::open(path)?;
    let mut buf = [0u8; 256];
    let mut last = b'\n';
    let result = loop {
        let n = match fs::read(fd, &mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(e),
        };
        let text: String =
            buf[..n].iter().map(|&b| if b == b'\n' || (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
        kprint!("{text}");
        last = buf[n - 1];
    };
    if last != b'\n' {
        kprintln!();
    }
    fs::close(fd)?;
    result
}

fn hexdump(_: &mut Shell, args: &[&str]) {
    let Some(path) = args.get(1) else {
        kprintln!("usage: hexdump file");
        return;
    };
    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(e) => {
            kprintln!("hexdump: {path}: {e}");
            return;
        }
    };
    // 40 桁の画面に合わせて 1 行 8 バイト
    let mut buf = [0u8; 8];
    let mut offset = 0u32;
    while let Ok(n) = fs::read(fd, &mut buf) {
        if n == 0 {
            break;
        }
        let hex: String = buf[..n].iter().map(|b| format!("{b:02x} ")).collect();
        let text: String = buf[..n].iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
        kprintln!("{offset:05x} {hex:24}{text}");
        offset += n as u32;
    }
    let _ = fs::close(fd);
}

fn write(_: &mut Shell, args: &[&str]) {
    let Some(path) = args.get(1) else {
        kprintln!("usage: write file text...");
        return;
    };
    let mut text = args[2..].join(" ");
    text.push('\n');
    if let Err(e) = fs::write_file(path, text.as_bytes()) {
        kprintln!("write: {path}: {e}");
    }
}

fn append(_: &mut Shell, args: &[&str]) {
    let Some(path) = args.get(1) else {
        kprintln!("usage: append file text...");
        return;
    };
    let mut text = args[2..].join(" ");
    text.push('\n');
    let r = fs::open_with(path, fs::OPEN_WRITE | fs::OPEN_CREATE | fs::OPEN_APPEND).and_then(|fd| {
        let r = fs::write(fd, text.as_bytes());
        fs::close(fd)?;
        r
    });
    if let Err(e) = r {
        kprintln!("append: {path}: {e}");
    }
}

fn cp(_: &mut Shell, args: &[&str]) {
    let [_, from, to] = args else {
        kprintln!("usage: cp from to");
        return;
    };
    match fs::read_file(from) {
        Ok(data) => {
            if let Err(e) = fs::write_file(to, &data) {
                kprintln!("cp: {to}: {e}");
            }
        }
        Err(e) => kprintln!("cp: {from}: {e}"),
    }
}

fn rm(_: &mut Shell, args: &[&str]) {
    let Some(path) = args.get(1) else {
        kprintln!("usage: rm path");
        return;
    };
    if let Err(e) = fs::remove(path) {
        kprintln!("rm: {path}: {e}");
    }
}

fn mkdir(_: &mut Shell, args: &[&str]) {
    let Some(path) = args.get(1) else {
        kprintln!("usage: mkdir dir");
        return;
    };
    if let Err(e) = fs::mkdir(path) {
        kprintln!("mkdir: {path}: {e}");
    }
}

fn df(_: &mut Shell, _: &[&str]) {
    match fs::space() {
        Ok((free, total)) => kprintln!("{} KB free of {} KB", free / 1024, total / 1024),
        Err(e) => kprintln!("df: {e}"),
    }
    let dirty = fs::dirty_sectors();
    if dirty > 0 {
        kprintln!("{dirty} sectors not saved (use sync)");
    }
}

fn sync(_: &mut Shell, _: &[&str]) {
    match fs::sync() {
        Ok(0) => kprintln!("sync: nothing to save"),
        Ok(n) => kprintln!("sync: saved {n} sectors"),
        Err(e) => kprintln!("sync: {e}"),
    }
}

fn cd(_: &mut Shell, args: &[&str]) {
    let path = args.get(1).copied().unwrap_or("/");
    if let Err(e) = fs::chdir(path) {
        kprintln!("cd: {path}: {e}");
    }
}

fn pwd(_: &mut Shell, _: &[&str]) {
    kprintln!("{}", fs::cwd());
}

fn mem(_: &mut Shell, _: &[&str]) {
    let (free, total) = pmm::stats();
    kprintln!("pages: {} KB free / {} KB", free * 4, total * 4);
    let (hfree, htotal, blocks) = heap::stats();
    kprintln!("heap: {hfree} / {htotal} bytes free, {blocks} blocks");
}

fn ps(_: &mut Shell, _: &[&str]) {
    kprintln!("  ID NAME       STATE    TICKS");
    for t in sched::tasks() {
        kprintln!("{:4} {:10} {:8} {}", t.id, t.name, format!("{:?}", t.state), t.ticks);
    }
}

fn date(_: &mut Shell, _: &[&str]) {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    match crate::drivers::rtc::now() {
        Some(t) => kprintln!("{} {}", t, DAYS[t.weekday() as usize]),
        None => kprintln!("date: no clock"),
    }
}

fn hwclock(_: &mut Shell, _: &[&str]) {
    match crate::drivers::rtc::read_cmos() {
        Ok(t) => kprintln!("cmos: {t}"),
        Err(raw) => kprintln!("hwclock: invalid time {raw:02x?}"),
    }
}

fn uptime(_: &mut Shell, _: &[&str]) {
    let t = pit::ticks();
    kprintln!("up {}.{:02} s ({} ticks)", t / pit::TICK_HZ, t % pit::TICK_HZ, t);
}

fn sleep(_: &mut Shell, args: &[&str]) {
    let ms: u32 = match args.get(1).map(|s| s.parse()) {
        None => 500,
        Some(Ok(ms)) => ms,
        Some(Err(_)) => {
            kprintln!("usage: sleep [ms]");
            return;
        }
    };
    let start = pit::ticks();
    pit::sleep_ms(ms);
    let elapsed = pit::ticks().wrapping_sub(start);
    let expected = (ms * pit::TICK_HZ).div_ceil(1000);
    let ok = elapsed >= expected && elapsed <= expected + expected / 5 + 2;
    kprintln!("slept {} ticks: {}", elapsed, if ok { "sleep ok" } else { "sleep NG" });
}

fn term(_: &mut Shell, _: &[&str]) {
    match crate::gfx::console::open_terminal(None) {
        Some(_) => {}
        None => kprintln!("term: too many terminals"),
    }
}

fn history(shell: &mut Shell, _: &[&str]) {
    for (i, h) in shell.editor.history().enumerate() {
        kprintln!("{:3} {h}", i + 1);
    }
}

static DEMO_RUNNING: AtomicBool = AtomicBool::new(false);

/// 3 つのスレッドが画面の右上にそれぞれのカウンタを描き続ける（5 秒で終わる）
fn demo(_: &mut Shell, _: &[&str]) {
    if DEMO_RUNNING.swap(true, Ordering::Relaxed) {
        kprintln!("demo is already running");
        return;
    }
    let colors = [palette::LIGHT_RED, palette::LIGHT_GREEN, palette::LIGHT_CYAN];
    let tty = sched::current_tty();
    let ids: Vec<_> = (0..3)
        .map(|i| {
            task::spawn(&format!("counter{i}"), move || {
                let start = pit::ticks();
                let mut n = 0u32;
                while pit::ticks().wrapping_sub(start) < 5 * pit::TICK_HZ {
                    n += 1;
                    let text = format!("T{i}:{n:6}");
                    let mut s = SCREEN.lock();
                    let Some((w, _)) = s.window_size(tty) else { break };
                    // 端末の右上に並べる
                    let x = (w - text.len() * FONT_8X8.width) as i32;
                    let y = (i * FONT_8X8.height) as i32;
                    if let Some(mut surf) = s.window_surface(tty) {
                        surf.draw_text(&FONT_8X8, x, y, &text, colors[i], Some(palette::BLACK));
                    }
                    s.mark_window_dirty(tty, y as usize, y as usize + FONT_8X8.height);
                    s.present();
                    drop(s);
                    // スレッドごとに速さを変える（1 つは休まない）
                    if i > 0 {
                        pit::sleep_ms(10 * i as u32);
                    }
                }
            })
        })
        .collect();
    task::spawn("demo-wait", move || {
        for id in ids {
            task::join(id);
        }
        DEMO_RUNNING.store(false, Ordering::Relaxed);
        klog!("demo finished");
    });
    kprintln!("demo started");
}

fn beep(_: &mut Shell, args: &[&str]) {
    let freq = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(880);
    let ms = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    crate::drivers::speaker::play(freq, ms);
    pit::sleep_ms(ms);
}

fn reboot(_: &mut Shell, _: &[&str]) {
    kprintln!("rebooting...");
    power::reboot();
}

fn divzero(_: &mut Shell, _: &[&str]) {
    // Rust の割り算はゼロ除算を事前に検査して panic するので、CPU の例外を起こすには div 命令を直接使う
    // SAFETY: わざと #DE を起こす（例外ハンドラの確認用）
    unsafe { asm!("div {0}", in(reg) 0u32, inout("eax") 1u32 => _, inout("edx") 0u32 => _) };
}

fn nullptr(_: &mut Shell, _: &[&str]) {
    // SAFETY: わざと 0 番地を読んでページフォルトを起こす（ページングの確認用）
    let v = unsafe { core::ptr::read_volatile(core::ptr::null::<u32>()) };
    kprintln!("read {v:#x} from address 0 (should not happen)");
}

fn overflow(_: &mut Shell, _: &[&str]) {
    kprintln!("overflowing the stack...");
    recurse(0);
}

/// スタックを使い切るまで再帰する（ガードページと二重フォルト処理の確認用）
#[inline(never)]
#[allow(unconditional_recursion)]
fn recurse(depth: u32) -> u32 {
    let buf = [depth as u8; 512];
    let r = recurse(depth + 1);
    // 再帰の後で buf を使うので、末尾呼び出しの最適化で消えない
    core::hint::black_box(&buf)[0] as u32 + r
}

fn panic_cmd(_: &mut Shell, _: &[&str]) {
    panic!("panic requested from the shell");
}

fn net_cmd(_: &mut Shell, _: &[&str]) {
    use crate::net::OptIp;
    let Some(i) = crate::net::info() else {
        kprintln!("net: no network card");
        return;
    };
    kprintln!("card: {} at {:#x} irq {}", i.card, i.io, i.irq);
    kprintln!("mac:  {}", i.mac);
    match i.config {
        Some(c) => {
            kprintln!("ip:   {}/{} ({})", c.addr, c.prefix, if i.dhcp { "dhcp" } else { "static" });
            kprintln!("gw:   {}", OptIp(c.gateway));
            kprintln!("dns:  {}", OptIp(c.dns));
        }
        None => kprintln!("ip:   (waiting for DHCP)"),
    }
    if let Some(ntp) = i.settings.ntp {
        kprintln!("ntp:  {ntp}");
    }
    let s = i.stats;
    kprintln!("rx:   {} packets, {} bytes ({} dropped)", s.rx_frames, s.rx_bytes, s.rx_dropped);
    kprintln!("tx:   {} packets, {} bytes", s.tx_frames, s.tx_bytes);
    for (ip, mac) in crate::net::arp_entries() {
        kprintln!("arp:  {ip} is at {mac}");
    }
    let socks = crate::net::socket_list();
    kprintln!("sockets: {}", socks.len());
    for so in socks {
        let remote = so.remote.map(|(ip, p)| format!("{ip}:{p}")).unwrap_or_default();
        kprintln!("  {} {} {} {} r{} s{}", so.kind, so.local_port, remote, so.state, so.recv_queued, so.send_queued);
    }
}

fn arp(_: &mut Shell, args: &[&str]) {
    let Some(arg) = args.get(1) else {
        for (ip, mac) in crate::net::arp_entries() {
            kprintln!("{ip} is at {mac}");
        }
        return;
    };
    let Some(ip) = vibe_net::Ipv4::parse(arg) else {
        kprintln!("arp: bad address: {arg}");
        return;
    };
    match crate::net::arp_resolve(ip, 3000) {
        Ok(mac) => kprintln!("{ip} is at {mac}"),
        Err(e) => kprintln!("arp: {ip}: {}", vibe_abi::err::name(e)),
    }
}

fn pci(_: &mut Shell, _: &[&str]) {
    use crate::drivers::pci;
    if !pci::present() {
        kprintln!("pci: no PCI bus");
        return;
    }
    for d in pci::scan() {
        let mut res = String::new();
        if let Some(io) = d.io_base(0) {
            res = format!(" io {io:#x}");
        } else if let Some(m) = d.mem_base(0) {
            res = format!(" mem {m:#x}");
        }
        if d.irq != 0 && d.irq != 0xFF {
            res.push_str(&format!(" irq {}", d.irq));
        }
        kprintln!(
            "{:02x}:{:02x}.{} {:04x}:{:04x} {}{}",
            d.bus,
            d.dev,
            d.func,
            d.vendor,
            d.device,
            d.class_name(),
            res
        );
    }
}
