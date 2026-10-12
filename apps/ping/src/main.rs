//! ping: ICMP のエコー要求を送り、返事までの時間を表示する
//!
//! ping [-c 回数] [-i 間隔ms] [-s データのバイト数] ホスト
//! 回数を指定しなければ Ctrl+C を押すまで続け、最後に集計を出す。

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

use vibe_rt::abi::{SOCK_ICMP, TICK_HZ, err};
use vibe_rt::net::{self, Socket};
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

const MS_PER_TICK: u32 = 1000 / TICK_HZ;
const REPLY_TIMEOUT_MS: u32 = 1000;

fn usage() -> i32 {
    println!("usage: ping [-c count] [-i interval_ms] [-s size] host");
    2
}

fn main() -> i32 {
    let mut count: Option<u32> = None;
    let mut interval = 1000u32;
    let mut size = 32usize;
    let mut host = None;
    let mut args = vibe_rt::args().skip(1);
    while let Some(a) = args.next() {
        let mut num = |name: &str| match args.next().and_then(|v| v.parse::<u32>().ok()) {
            Some(v) => Ok(v),
            None => {
                println!("ping: {name} needs a number");
                Err(())
            }
        };
        match a {
            "-c" => match num("-c") {
                Ok(v) => count = Some(v.max(1)),
                Err(()) => return usage(),
            },
            "-i" => match num("-i") {
                Ok(v) => interval = v.max(100),
                Err(()) => return usage(),
            },
            "-s" => match num("-s") {
                Ok(v) if v as usize <= 1400 => size = v as usize,
                _ => return usage(),
            },
            h if host.is_none() && !h.starts_with('-') => host = Some(h),
            _ => return usage(),
        }
    }
    let Some(host) = host else { return usage() };
    let ip = match net::resolve(host) {
        Ok(ip) => ip,
        Err(e) => {
            println!("ping: {host}: {}", e.message());
            return 1;
        }
    };
    let sock = match Socket::new(SOCK_ICMP) {
        Ok(s) => s,
        Err(e) => {
            println!("ping: {}", net::err_name(e));
            return 1;
        }
    };
    net::catch_ctrl_c();
    println!("PING {host} ({ip}): {size} data bytes");

    let (mut sent, mut received) = (0u32, 0u32);
    let mut times: Vec<u32> = Vec::new();
    let mut buf = [0u8; 1500];
    let mut seq: u16 = 0;
    'outer: loop {
        seq = seq.wrapping_add(1);
        // エコー要求: 種類 8, コード 0, チェックサム・識別子（カーネルが入れる）, 通し番号, データ
        let mut msg = Vec::with_capacity(8 + size);
        msg.extend_from_slice(&[8, 0, 0, 0, 0, 0]);
        msg.extend_from_slice(&seq.to_be_bytes());
        msg.extend((0..size).map(|i| b'a' + (i % 26) as u8));
        let start = sys::ticks();
        let r = net::sendto(sock.0, &msg, ip, 0);
        if r < 0 {
            // 送れない（WAN を許していない・経路が無いなど）: 続けても同じなので終える
            if -r != err::EINTR {
                println!("ping: {host}: {}", net::err_name(-r));
            }
            break;
        }
        sent += 1;
        // 返事（この通し番号のもの）を待つ
        loop {
            let waited = sys::ticks().wrapping_sub(start) * MS_PER_TICK;
            if waited >= REPLY_TIMEOUT_MS {
                println!("Request timeout for icmp_seq {seq}");
                break;
            }
            match net::recvfrom(sock.0, &mut buf, REPLY_TIMEOUT_MS - waited) {
                Ok((n, from, _)) if n >= 8 => {
                    let (kind, rseq) = (buf[0], u16::from_be_bytes([buf[6], buf[7]]));
                    if kind == 0 && rseq == seq {
                        let ms = sys::ticks().wrapping_sub(start) * MS_PER_TICK;
                        received += 1;
                        times.push(ms);
                        println!("{} bytes from {from}: icmp_seq={seq} time={ms} ms", n);
                        break;
                    }
                    if kind == 3 {
                        println!("From {from}: Destination unreachable");
                        break;
                    }
                }
                Ok(_) => {}
                Err(err::ETIMEDOUT) => {}
                Err(err::EINTR) => break 'outer,
                Err(e) => {
                    println!("ping: receive: {}", net::err_name(e));
                    break 'outer;
                }
            }
        }
        if net::ctrl_c_pressed() || count.is_some_and(|c| sent >= c) {
            break;
        }
        // 次を送るまで待つ（Ctrl+C で抜ける）
        while sys::ticks().wrapping_sub(start) * MS_PER_TICK < interval {
            if net::ctrl_c_pressed() {
                break 'outer;
            }
            sys::sleep(MS_PER_TICK * 5);
        }
    }

    println!("--- {host} ping statistics ---");
    let loss = if sent == 0 { 0 } else { (sent - received) * 100 / sent };
    println!("{sent} packets transmitted, {received} received, {loss}% packet loss");
    if !times.is_empty() {
        let min = times.iter().min().unwrap();
        let max = times.iter().max().unwrap();
        let avg = times.iter().sum::<u32>() / times.len() as u32;
        println!("rtt min/avg/max = {min}/{avg}/{max} ms");
    }
    if received > 0 { 0 } else { 1 }
}
