//! ローダから渡されるブート情報。レイアウトは boot/bootinfo.inc と一致させる。

use core::mem::{offset_of, size_of};

pub const BOOT_MAGIC: u32 = 0x4542_4956; // "VIBE"
pub const MMAP_MAX: usize = 32;

/// E820 のメモリ領域の種類
pub const E820_USABLE: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct E820Entry {
    pub base: u64,
    pub len: u64,
    pub kind: u32,
    pub acpi: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BootInfo {
    pub magic: u32,
    pub boot_drive: u32,
    pub mmap_count: u32,
    pub mmap: [E820Entry; MMAP_MAX],
    pub ramdisk_addr: u32,
    pub ramdisk_size: u32,
    pub vram_addr: u32,
    pub screen_w: u16,
    pub screen_h: u16,
    pub kernel_addr: u32,
    pub kernel_size: u32,
    /// 画面 1 行のバイト数（モード 12h は 1 枚の面の 1 行）
    pub pitch: u32,
    /// 1 ピクセルのビット数（8 = モード 13h、4 = モード 12h（4 枚の面）、32 = VESA の直接色）
    pub bpp: u32,
}

const _: () = {
    assert!(size_of::<E820Entry>() == 24);
    assert!(offset_of!(BootInfo, mmap) == 0x00C);
    assert!(offset_of!(BootInfo, ramdisk_addr) == 0x30C);
    assert!(offset_of!(BootInfo, vram_addr) == 0x314);
    assert!(offset_of!(BootInfo, screen_w) == 0x318);
    assert!(offset_of!(BootInfo, kernel_addr) == 0x31C);
    assert!(offset_of!(BootInfo, kernel_size) == 0x320);
    assert!(offset_of!(BootInfo, pitch) == 0x324);
    assert!(offset_of!(BootInfo, bpp) == 0x328);
    assert!(size_of::<BootInfo>() == 0x32C);
};

impl BootInfo {
    pub fn memory_map(&self) -> &[E820Entry] {
        &self.mmap[..(self.mmap_count as usize).min(MMAP_MAX)]
    }
}

/// ローダが 0x500 に置いた構造体の写し。0 番地のページはヌルポインタ検出のためページングで
/// 対応付けないので、ページングを有効にする前に写しておく。
static BOOT_INFO: crate::sync::BootCell<BootInfo> = crate::sync::BootCell::new(BootInfo {
    magic: 0,
    boot_drive: 0,
    mmap_count: 0,
    mmap: [E820Entry { base: 0, len: 0, kind: 0, acpi: 0 }; MMAP_MAX],
    ramdisk_addr: 0,
    ramdisk_size: 0,
    vram_addr: 0,
    screen_w: 0,
    screen_h: 0,
    kernel_addr: 0,
    kernel_size: 0,
    pitch: 0,
    bpp: 0,
});

/// kmain の最初に 1 回だけ（割り込み禁止・ページング前に）呼ぶ。写しへの参照を返す
pub fn set(loader_info: &BootInfo) -> &'static BootInfo {
    // SAFETY: 起動直後でほかに誰も読んでいない
    unsafe {
        *BOOT_INFO.as_ptr() = *loader_info;
        &*BOOT_INFO.as_ptr()
    }
}

/// ブート情報（カーネル内の写し）
#[allow(dead_code)]
pub fn get() -> &'static BootInfo {
    // SAFETY: set の後は書き換えない
    let info = unsafe { &*BOOT_INFO.as_ptr() };
    assert!(info.magic == BOOT_MAGIC, "boot info is not set yet");
    info
}
