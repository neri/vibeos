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
mod drivers;
mod gfx;
#[cfg(feature = "qemu-test")]
mod ktest;
mod mm;
mod monitor;
mod panic;
mod sync;
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

extern "C" fn kmain(magic: u32, info: &'static BootInfo) -> ! {
    drivers::serial::init();
    klog!("VibeOS kernel starting");
    if magic != BOOT_MAGIC {
        panic!("bad boot magic {magic:#010x}");
    }

    drivers::vga::init(info);
    gfx::console::init();

    gfx::console::set_color(palette::YELLOW, palette::BLACK);
    kprintln!("VibeOS");
    gfx::console::set_color(palette::LIGHT_GRAY, palette::BLACK);
    kprintln!("Hello from Rust!");
    kprintln!();

    check_cpu();
    print_memory(info);
    mm::init(info);

    let stack_top = (&raw const BOOT_STACK) as u32 + BOOT_STACK_SIZE as u32;
    arch::gdt::init(stack_top);
    arch::idt::init();
    arch::pic::init();
    drivers::pit::init();
    drivers::keyboard::init();
    arch::pic::enable_nmi();
    // SAFETY: IDT・PIC・各ハンドラの準備ができた
    unsafe { arch::cpu::enable_interrupts() };
    klog!("interrupts enabled");

    klog!("VIBEOS READY");

    #[cfg(feature = "qemu-test")]
    ktest::run_all();

    #[cfg(not(feature = "qemu-test"))]
    monitor::run()
}

fn check_cpu() {
    match arch::cpu::identify() {
        Some(cpu) => {
            kprintln!("cpu: {} family {} model {} stepping {}", cpu.vendor_str(), cpu.family, cpu.model, cpu.stepping);
            if cpu.family < 6 {
                panic!("VibeOS needs an i686 (Pentium Pro / II) or newer CPU");
            }
        }
        None => panic!("VibeOS needs an i686 (Pentium Pro / II) or newer CPU (no CPUID)"),
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
