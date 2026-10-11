//! VibeOS カーネル
//!
//! ローダから 32bit プロテクトモードで `_start`（0x100000）に飛んでくる。
//! EAX = BOOT_MAGIC, EBX = ブート情報構造体のアドレス。

#![no_std]
#![no_main]

#[macro_use]
mod print;

mod arch;
mod bootinfo;
mod drivers;
mod gfx;
mod panic;
mod sync;

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
    kprintln!("kernel  {:08x} +{:x}", info.kernel_addr, info.kernel_size);
    kprintln!("ramdisk {:08x} +{:x}", info.ramdisk_addr, info.ramdisk_size);
    kprintln!("memory map:");
    let mut usable = 0u64;
    for e in info.memory_map() {
        kprintln!(" {:09x}-{:09x} {}", e.base, e.base + e.len, e.kind);
        if e.kind == E820_USABLE {
            usable += e.len;
        }
    }
    kprintln!("usable: {} KB", usable / 1024);

    klog!("VIBEOS READY");
    arch::cpu::halt_forever()
}
