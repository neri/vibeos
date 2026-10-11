//! Bochs で動かすための部分
//!
//! Bochs（Debian の 2.7）は画面を出さずに動かす方法が無いので、VNC（RFB）サーバとして画面を
//! 出させ、ここで最小限の RFB クライアントとしてつなぐ（つながないと 30 秒で終了してしまう）。
//! キー入力は RFB のキーイベントで送る。シリアルは Bochs がファイルに書くので、それを読む。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::Result;

const RFB_PORT: u16 = 5900;

/// Bochs の画面（RFB）への接続
pub struct Rfb {
    stream: TcpStream,
}

/// Bochs を起動し、RFB でつなぎ、シリアルのファイルを `serial` に読み込み続けるスレッドを作る
pub fn start(img: &Path, out: &Path, name: &str, serial: Arc<Mutex<Vec<u8>>>) -> Result<(Child, Rfb)> {
    let serial_path = out.join(format!("{name}.bochs-serial.log"));
    let _ = fs::remove_file(&serial_path);
    let console = fs::File::create(out.join(format!("{name}.bochs.out"))).map_err(|e| e.to_string())?;
    let child = Command::new("bochs")
        .args(["-q", "-f", "tools/bochs/bochsrc.txt", "-rc", "tools/bochs/continue.rc"])
        .env("BOCHS_FLOPPY", img)
        .env("BOCHS_SERIAL", &serial_path)
        .env("BOCHS_LOG", out.join(format!("{name}.bochs.log")))
        .env("BOCHS_DISPLAY", "rfb")
        .env("BOCHS_DISPLAY_OPTIONS", "timeout=30")
        .stdin(Stdio::null())
        .stdout(Stdio::from(console.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(console))
        .spawn()
        .map_err(|e| format!("cannot start bochs: {e}"))?;

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut rfb = loop {
        match TcpStream::connect(("127.0.0.1", RFB_PORT)) {
            Ok(s) => break s,
            Err(e) if Instant::now() > deadline => return Err(format!("cannot connect to Bochs RFB: {e}")),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    };
    handshake(&mut rfb)?;

    // サーバから来るもの（画面の更新など）は読み捨てる（読まないとサーバが止まることがある）
    let mut reader = rfb.try_clone().map_err(|e| e.to_string())?;
    thread::spawn(move || {
        let mut buf = [0u8; 65536];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    });

    // シリアルのファイルを追いかける
    thread::spawn(move || {
        let mut pos = 0;
        loop {
            if let Ok(data) = fs::read(&serial_path) {
                if data.len() > pos {
                    serial.lock().unwrap().extend_from_slice(&data[pos..]);
                    pos = data.len();
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
    Ok((child, Rfb { stream: rfb }))
}

fn read_exact(s: &mut TcpStream, n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).map_err(|e| format!("RFB read: {e}"))?;
    Ok(buf)
}

/// RFB 3.3 の最小限の手続き（認証なし）
fn handshake(s: &mut TcpStream) -> Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let version = read_exact(s, 12)?;
    if !version.starts_with(b"RFB 003.") {
        return Err(format!("unexpected RFB version {:?}", String::from_utf8_lossy(&version)));
    }
    s.write_all(b"RFB 003.003\n").map_err(|e| e.to_string())?;
    let security = u32::from_be_bytes(read_exact(s, 4)?.try_into().unwrap());
    if security != 1 {
        return Err(format!("RFB security type {security} is not supported"));
    }
    s.write_all(&[1]).map_err(|e| e.to_string())?; // 共有
    let init = read_exact(s, 24)?;
    let name_len = u32::from_be_bytes(init[20..24].try_into().unwrap()) as usize;
    read_exact(s, name_len)?;
    s.set_read_timeout(None).unwrap();
    Ok(())
}

impl Rfb {
    fn key_event(&mut self, keysym: u32, down: bool) -> Result<()> {
        let mut msg = vec![4u8, down as u8, 0, 0];
        msg.extend_from_slice(&keysym.to_be_bytes());
        self.stream.write_all(&msg).map_err(|e| format!("RFB write: {e}"))
    }

    /// QEMU の qcode 名の並び（例: ["shift", "a"]）を押して離す
    pub fn send_keys(&mut self, qcodes: &[&str]) -> Result<()> {
        let syms: Vec<u32> =
            qcodes.iter().map(|q| keysym(q).ok_or_else(|| format!("no keysym for {q}"))).collect::<Result<_>>()?;
        for &k in &syms {
            self.key_event(k, true)?;
            thread::sleep(Duration::from_millis(20));
        }
        for &k in syms.iter().rev() {
            self.key_event(k, false)?;
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

/// QEMU の qcode 名 → X11 の keysym
fn keysym(q: &str) -> Option<u32> {
    if q.len() == 1 {
        let c = q.as_bytes()[0];
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    Some(match q {
        "spc" => 0x20,
        "ret" => 0xff0d,
        "backspace" => 0xff08,
        "tab" => 0xff09,
        "esc" => 0xff1b,
        "up" => 0xff52,
        "down" => 0xff54,
        "left" => 0xff51,
        "right" => 0xff53,
        "home" => 0xff50,
        "end" => 0xff57,
        "delete" => 0xffff,
        "caps_lock" => 0xffe5,
        "shift" => 0xffe1,
        "ctrl" => 0xffe3,
        "alt" => 0xffe9,
        "minus" => b'-' as u32,
        "equal" => b'=' as u32,
        "bracket_left" => b'[' as u32,
        "bracket_right" => b']' as u32,
        "backslash" => b'\\' as u32,
        "semicolon" => b';' as u32,
        "apostrophe" => b'\'' as u32,
        "grave_accent" => b'`' as u32,
        "comma" => b',' as u32,
        "dot" => b'.' as u32,
        "slash" => b'/' as u32,
        f if f.starts_with('f') => 0xffbe + f[1..].parse::<u32>().ok()?.checked_sub(1)?,
        _ => return None,
    })
}
