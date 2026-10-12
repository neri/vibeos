pub mod fdc;
pub mod keyboard;
pub mod mouse;
pub mod ne2000;
pub mod nic;
pub mod pci;
pub mod pit;
pub mod power;
pub mod ps2;
pub mod ramdisk;
pub mod rtc;
pub mod rtl8139;
pub mod serial;
pub mod speaker;
pub mod vga;

#[cfg(feature = "qemu-test")]
pub mod qemu;
