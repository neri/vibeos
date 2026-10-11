pub mod fdc;
pub mod keyboard;
pub mod mouse;
pub mod pit;
pub mod power;
pub mod ps2;
pub mod ramdisk;
pub mod serial;
pub mod speaker;
pub mod vga;

#[cfg(feature = "qemu-test")]
pub mod qemu;
