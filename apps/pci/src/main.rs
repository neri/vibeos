//! pci: PCI の機器の一覧

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use vibe_rt::{abi, eprintln, println, sys};

vibe_rt::entry!(main);

fn main() -> i32 {
    let list = match sys::pci_list() {
        Ok(l) => l,
        Err(abi::err::ENODEV) => {
            eprintln!("pci: no PCI bus");
            return 1;
        }
        Err(e) => {
            eprintln!("pci: {}", abi::err::name(e));
            return 1;
        }
    };
    for d in list {
        let mut res = String::new();
        if d.io != 0 {
            res = format!(" io {:#x}", d.io);
        } else if d.mem != 0 {
            res = format!(" mem {:#x}", d.mem);
        }
        if d.irq != 0 && d.irq != 0xFF {
            res.push_str(&format!(" irq {}", d.irq));
        }
        let class = abi::pci_class_name(d.class, d.subclass);
        println!("{:02x}:{:02x}.{} {:04x}:{:04x} {}{}", d.bus, d.dev, d.func, d.vendor, d.device, class, res);
    }
    0
}
