//! arp [アドレス]: ARP で MAC アドレスを引く（無ければ表を出す）

#![no_std]
#![no_main]

use vibe_rt::net::{self, Ipv4};
use vibe_rt::{abi, eprintln, println};

vibe_rt::entry!(main);

fn main() -> i32 {
    let Some(arg) = vibe_rt::args().nth(1) else {
        for e in net::arp_list() {
            println!("{} is at {}", Ipv4(e.ip), net::Mac(e.mac));
        }
        return 0;
    };
    let Some(ip) = Ipv4::parse(arg) else {
        eprintln!("arp: bad address: {arg}");
        return 2;
    };
    match net::arp_resolve(ip, 3000) {
        Ok(mac) => {
            println!("{ip} is at {mac}");
            0
        }
        Err(e) => {
            eprintln!("arp: {ip}: {}", abi::err::name(e));
            1
        }
    }
}
