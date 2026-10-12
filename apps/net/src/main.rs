//! net: ネットワークカード・アドレス・送受信の数・ARP の表・ソケット

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use vibe_rt::abi::{self, NET_CONFIGURED, NET_DHCP};
use vibe_rt::net::{self, Ipv4, Mac};
use vibe_rt::{eprintln, println};

vibe_rt::entry!(main);

/// 0.0.0.0 なら "-"
fn opt_ip(ip: [u8; 4]) -> String {
    if ip == [0; 4] { String::from("-") } else { format!("{}", Ipv4(ip)) }
}

fn main() -> i32 {
    let i = match net::net_info() {
        Ok(i) => i,
        Err(abi::err::ENODEV) => {
            eprintln!("net: no network card");
            return 1;
        }
        Err(e) => {
            eprintln!("net: {}", abi::err::name(e));
            return 1;
        }
    };
    println!("card: {} at {:#x} irq {}", abi::fixed_str(&i.card), i.io, i.irq);
    println!("mac:  {}", Mac(i.mac));
    if i.flags & NET_CONFIGURED != 0 {
        let prefix = Ipv4(i.netmask).prefix_len().unwrap_or(0);
        let how = if i.flags & NET_DHCP != 0 { "dhcp" } else { "static" };
        println!("ip:   {}/{} ({how})", Ipv4(i.addr), prefix);
        println!("gw:   {}", opt_ip(i.gateway));
        println!("dns:  {}", opt_ip(i.dns));
    } else {
        println!("ip:   (waiting for DHCP)");
    }
    if i.ntp != [0; 4] {
        println!("ntp:  {}", Ipv4(i.ntp));
    }
    println!("rx:   {} packets, {} bytes ({} dropped)", i.rx_packets, i.rx_bytes, i.rx_dropped);
    println!("tx:   {} packets, {} bytes", i.tx_packets, i.tx_bytes);
    for e in net::arp_list() {
        println!("arp:  {} is at {}", Ipv4(e.ip), Mac(e.mac));
    }
    let socks = net::sock_list();
    println!("sockets: {}", socks.len());
    for so in socks {
        let remote = if so.remote == [0; 4] { String::new() } else { format!("{}:{}", Ipv4(so.remote), so.remote_port) };
        println!(
            "  {} {} {} {} r{} s{}",
            abi::fixed_str(&so.kind),
            so.local_port,
            remote,
            abi::fixed_str(&so.state),
            so.recv_queued,
            so.send_queued
        );
    }
    0
}
