//! VibeOS カーネル
//!
//! ローダから 32bit プロテクトモードで `_start`（0x100000）に飛んでくる。
//! EAX = BOOT_MAGIC, EBX = ブート情報構造体のアドレス。

#![no_std]
#![no_main]
// テスト用ビルドではモニタなど通常の起動でだけ使うものが未使用になる
#![cfg_attr(feature = "qemu-test", allow(dead_code))]

extern crate alloc;

#[macro_use]
mod print;

mod arch;
mod bootinfo;
mod clipboard;
mod demo;
mod drivers;
mod fs;
mod gfx;
mod gui;
mod input;
mod io;
#[cfg(feature = "qemu-test")]
mod ktest;
mod mm;
mod net;
mod panic;
mod pipe;
mod process;
mod shell;
mod sync;
mod syscall;
mod task;
mod util;

use bootinfo::{BOOT_MAGIC, BootInfo, E820_USABLE};
use vibe_gfx::palette;

const BOOT_STACK_SIZE: usize = 16 * 1024;

// エントリ: BSS をゼロクリア（実機のメモリは初期化されていない）してからスタックを用意し kmain へ
core::arch::global_asm!(
    ".section .text.entry, \"ax\"",
    ".global _start",
    "_start:",
    "    cli",
    "    cld",
    "    mov esi, eax",
    "    mov edx, ebx",
    "    mov edi, offset __bss_start",
    "    mov ecx, offset __bss_end",
    "    sub ecx, edi",
    "    xor eax, eax",
    "    rep stosb",
    "    mov esp, offset {stack} + {stack_size}",
    "    xor ebp, ebp",
    // kmain の入口で ESP+4 が 16 の倍数になるように（データレイアウトの S128）
    "    sub esp, 8",
    "    push edx",
    "    push esi",
    "    call {kmain}",
    "2:  cli",
    "    hlt",
    "    jmp 2b",
    stack = sym BOOT_STACK,
    stack_size = const BOOT_STACK_SIZE,
    kmain = sym kmain,
);

#[repr(C, align(16))]
struct Stack([u8; BOOT_STACK_SIZE]);

// アセンブラから `sym` で番地を使うだけで、Rust からは参照を作らない
#[used]
static mut BOOT_STACK: Stack = Stack([0; BOOT_STACK_SIZE]);

extern "C" fn kmain(magic: u32, loader_info: &'static BootInfo) -> ! {
    drivers::serial::init();
    klog!("VibeOS kernel starting");
    if magic != BOOT_MAGIC {
        panic!("bad boot magic {magic:#010x}");
    }
    // 0x500 にある元の構造体はページングを有効にすると読めなくなるので、写しを使う
    let info = bootinfo::set(loader_info);

    drivers::vga::init(info);
    gui::init();
    gfx::console::init();

    gfx::console::set_color(palette::YELLOW, palette::BLACK);
    kprintln!("VibeOS");
    gfx::console::set_color(palette::LIGHT_GRAY, palette::BLACK);
    kprintln!("Hello from Rust!");
    kprintln!();

    check_cpu();
    print_memory(info);
    mm::init(info);
    gui::init_windows();

    let stack_top = (&raw const BOOT_STACK) as u32 + BOOT_STACK_SIZE as u32;
    arch::gdt::init(stack_top);
    arch::idt::init();
    arch::pic::init();
    drivers::pit::init();
    drivers::rtc::init();
    let ps2 = drivers::ps2::init();
    if ps2.keyboard {
        drivers::keyboard::init();
    }
    if ps2.mouse {
        drivers::mouse::init();
    }
    arch::pic::enable_nmi();
    task::sched::init();
    fs::mount_root(info);
    if ps2.mouse {
        input::start_mouse();
    }
    gui::start_input_router();
    // SAFETY: IDT・PIC・各ハンドラ・スケジューラの準備ができた
    unsafe { arch::cpu::enable_interrupts() };
    klog!("interrupts enabled");
    // フロッピーの初期化は割り込みを待つので、割り込みを許可してから
    drivers::fdc::init();
    // なめらかなフォントとアイコン（デスクトップの飾り。ファイルシステムの後、デスクトップにする前。
    // ネットワークより先に読む: DHCP の知らせが VIBEOS READY より前に出ないように）
    gfx::fonts::load();
    gfx::icons::load();
    // ネットワーク（BOOT.CFG を読むのでファイルシステムの後。アドレスは裏で DHCP がもらう）
    net::init();

    klog!("VIBEOS READY");

    // ここから先はガードページ付きのスタックを持つタスクで動かす（起動時のスタックには無い）
    #[cfg(feature = "qemu-test")]
    task::spawn("ktest", || ktest::run_all());
    #[cfg(not(feature = "qemu-test"))]
    {
        // 画面が広ければデスクトップ（ウィンドウ）にして、コンソールを端末のウィンドウに合わせる
        if gui::start_desktop() {
            gfx::console::init();
            klog!("gui: desktop {}x{}", drivers::vga::width(), drivers::vga::height());
            gfx::wallpaper::load_from_config();
        }
        gfx::statusbar::start();
        task::spawn("shell", || shell::run_terminal(None));
    }
    task::exit()
}

fn check_cpu() {
    match arch::cpu::identify() {
        Some(cpu) => {
            kprintln!("cpu: {} {}/{}/{}", cpu.vendor_str(), cpu.family, cpu.model, cpu.stepping);
            if cpu.family < 4 {
                panic!("VibeOS needs a 486 or newer CPU");
            }
        }
        // CPUID の無い初期の 486（486 以降かどうかはローダが確かめている）
        None => kprintln!("cpu: no CPUID (early 486)"),
    }
}

fn print_memory(info: &BootInfo) {
    let mut usable = 0u64;
    for e in info.memory_map() {
        klog!("e820: {:09x}-{:09x} type {}", e.base, e.base + e.len, e.kind);
        if e.kind == E820_USABLE {
            usable += e.len;
        }
    }
    kprintln!("memory: {} KB usable", usable / 1024);
    klog!("kernel {:08x} +{:x}, ramdisk {:08x} +{:x}", info.kernel_addr, info.kernel_size, info.ramdisk_addr, info.ramdisk_size);
}
