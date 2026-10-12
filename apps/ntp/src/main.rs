//! ntp: SNTP で時刻を問い合わせて時計を合わせる（CMOS の時計にも書く）
//!
//! ntp [サーバ]   サーバを省くと BOOT.CFG の ntp=。NTP の時刻（UTC）に BOOT.CFG の tz= を足して
//! 現地時刻にする（PC の時計はふつう現地時刻）

#![no_std]
#![no_main]

use vibe_rt::abi::{SOCK_DGRAM, err};
use vibe_rt::net::{self, Ipv4, Socket, sntp};
use vibe_rt::time::DateTime;
use vibe_rt::{println, sys};

vibe_rt::entry!(main);

const TIMEOUT_MS: u32 = 2000;
const TRIES: u32 = 3;

fn main() -> i32 {
    let args: alloc::vec::Vec<&str> = vibe_rt::args().collect();
    let info = match net::net_info() {
        Ok(i) => i,
        Err(e) => {
            println!("ntp: {}", net::err_name(e));
            return 1;
        }
    };
    let (server, name) = match args[..] {
        [_] if info.ntp != [0; 4] => (Ipv4(info.ntp), None),
        [_] => {
            println!("ntp: no NTP server (give one, or set ntp= in BOOT.CFG)");
            return 1;
        }
        [_, host] => match net::resolve(host) {
            Ok(ip) => (ip, Some(host)),
            Err(e) => {
                println!("ntp: {host}: {}", e.message());
                return 1;
            }
        },
        _ => {
            println!("usage: ntp [server]");
            return 2;
        }
    };
    let sock = match Socket::new(SOCK_DGRAM) {
        Ok(s) => s,
        Err(e) => {
            println!("ntp: {}", net::err_name(e));
            return 1;
        }
    };
    let mut buf = [0u8; 512];
    for _ in 0..TRIES {
        let tag = (sys::ticks() as u64) << 32 | sys::getpid() as u64;
        let r = net::sendto(sock.0, &sntp::build_request(tag), server, sntp::PORT);
        if r < 0 {
            println!("ntp: {}", net::err_name(-r));
            return 1;
        }
        let start = sys::ticks();
        loop {
            let waited = sys::ticks().wrapping_sub(start) * (1000 / vibe_rt::abi::TICK_HZ);
            if waited >= TIMEOUT_MS {
                break;
            }
            match net::recvfrom(sock.0, &mut buf, TIMEOUT_MS - waited) {
                Ok((n, from, port)) if from == server && port == sntp::PORT => {
                    match sntp::parse_response(&buf[..n], tag) {
                        Ok(utc) => return set_clock(utc, info.tz_minutes, server, name),
                        Err(sntp::SntpError::WrongReply) => continue,
                        Err(e) => {
                            println!("ntp: bad reply from {server}: {e:?}");
                            return 1;
                        }
                    }
                }
                Ok(_) => continue,
                Err(err::ETIMEDOUT) => break,
                Err(e) => {
                    println!("ntp: {}", net::err_name(e));
                    return 1;
                }
            }
        }
    }
    println!("ntp: no reply from {server}");
    1
}

fn set_clock(utc: u64, tz_minutes: i32, server: Ipv4, name: Option<&str>) -> i32 {
    let local = (utc as i64 + tz_minutes as i64 * 60).max(0) as u64;
    let before = sys::time() as i64;
    if net::set_time(local as u32) < 0 {
        println!("ntp: cannot set the clock");
        return 1;
    }
    let sign = if tz_minutes < 0 { '-' } else { '+' };
    let tz = tz_minutes.unsigned_abs();
    match name {
        Some(n) => println!("ntp: {n} ({server}) says {} UTC", DateTime::from_unix(utc)),
        None => println!("ntp: {server} says {} UTC", DateTime::from_unix(utc)),
    }
    println!(
        "ntp: clock set to {} (UTC{sign}{:02}:{:02}), was {} s off",
        DateTime::from_unix(local),
        tz / 60,
        tz % 60,
        local as i64 - before
    );
    0
}

extern crate alloc;
