//! host: DNS で名前のアドレス（A レコード）を引く
//!
//! host 名前 [DNS サーバ]   サーバを省くと BOOT.CFG の dns= か DHCP でもらったもの

#![no_std]
#![no_main]

use vibe_rt::net::{self, Ipv4, ResolveError};
use vibe_rt::println;

vibe_rt::entry!(main);

fn main() -> i32 {
    let args: alloc::vec::Vec<&str> = vibe_rt::args().collect();
    let (name, server) = match args[..] {
        [_, name] => (name, None),
        [_, name, server] => (name, Some(server)),
        _ => {
            println!("usage: host name [server]");
            return 2;
        }
    };
    let server = match server {
        Some(s) => match Ipv4::parse(s) {
            Some(ip) => ip,
            None => {
                println!("host: bad server address: {s}");
                return 2;
            }
        },
        None => match net::net_info() {
            Ok(info) if info.dns != [0; 4] => Ipv4(info.dns),
            Ok(_) => {
                println!("host: {}", ResolveError::NoServer.message());
                return 1;
            }
            Err(e) => {
                println!("host: {}", net::err_name(e));
                return 1;
            }
        },
    };
    match net::resolve_with(name, server, 2000, 3) {
        Ok(addrs) => {
            for a in addrs {
                println!("{name} has address {a}");
            }
            0
        }
        Err(e) => {
            println!("host: {name}: {}", e.message());
            1
        }
    }
}

extern crate alloc;
