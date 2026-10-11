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
}

const _: () = {
    assert!(size_of::<E820Entry>() == 24);
    assert!(offset_of!(BootInfo, mmap) == 0x00C);
    assert!(offset_of!(BootInfo, ramdisk_addr) == 0x30C);
    assert!(offset_of!(BootInfo, vram_addr) == 0x314);
    assert!(offset_of!(BootInfo, screen_w) == 0x318);
    assert!(offset_of!(BootInfo, kernel_addr) == 0x31C);
    assert!(offset_of!(BootInfo, kernel_size) == 0x320);
    assert!(size_of::<BootInfo>() == 0x324);
};

impl BootInfo {
    pub fn memory_map(&self) -> &[E820Entry] {
        &self.mmap[..(self.mmap_count as usize).min(MMAP_MAX)]
    }
}
