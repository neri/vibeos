pub mod keyboard;
pub mod pit;
pub mod ramdisk;
pub mod serial;
pub mod vga;

#[cfg(feature = "qemu-test")]
pub mod qemu;
