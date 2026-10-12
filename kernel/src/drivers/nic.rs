//! ネットワークカード（1 枚）をまとめて扱う: NE2000（ISA / PCI）か RTL8139（PCI）
//!
//! net スレッドはここの関数だけを使う。どのカードかは起動時の `probe` で決まる。

use core::sync::atomic::{AtomicU8, Ordering};

use vibe_net::Mac;
use vibe_net::config::Device;

use super::{ne2000, pci, rtl8139};
use crate::sync::IrqMutex;

const NONE: u8 = 0;
const NE2000: u8 = 1;
const RTL8139: u8 = 2;

static KIND: AtomicU8 = AtomicU8::new(NONE);

/// NE2000 の PCI 版（Realtek RTL8029。QEMU の ne2k_pci）
const NE2000_PCI: (u16, u16) = (0x10EC, 0x8029);

#[derive(Debug, Clone, Copy)]
pub struct CardInfo {
    pub name: &'static str,
    pub mac: Mac,
    pub io: u16,
    pub irq: u8,
}

static INFO: IrqMutex<Option<CardInfo>> = IrqMutex::new(None);

/// 設定に従ってカードを探して初期化する
pub fn probe(device: Device) -> Option<CardInfo> {
    let info = match device {
        Device::None => return None,
        Device::Ne2000 { io, irq } => ne2000::probe(io, irq).map(|mac| (NE2000, "NE2000", mac, io, irq)),
        Device::Auto => probe_pci().or_else(|| {
            let (io, irq) = (0x300, 10);
            ne2000::probe(io, irq).map(|mac| (NE2000, "NE2000", mac, io, irq))
        }),
    };
    let (kind, name, mac, io, irq) = info?;
    let info = CardInfo { name, mac, io, irq };
    KIND.store(kind, Ordering::Release);
    *INFO.lock() = Some(info);
    Some(info)
}

/// PCI のバスから知っているカードを探す
fn probe_pci() -> Option<(u8, &'static str, Mac, u16, u8)> {
    for d in pci::scan() {
        // BIOS が割り込みを決めていない（0 か 255）カードは使えない
        if !(1..16).contains(&d.irq) {
            continue;
        }
        match (d.vendor, d.device) {
            (rtl8139::VENDOR, rtl8139::DEVICE) => {
                if let Some((mac, io, irq)) = rtl8139::probe(&d) {
                    return Some((RTL8139, "RTL8139", mac, io, irq));
                }
            }
            NE2000_PCI => {
                let Some(io) = d.io_base(0) else { continue };
                d.enable(pci::COMMAND_IO);
                if let Some(mac) = ne2000::probe(io, d.irq) {
                    return Some((NE2000, "NE2000 (PCI)", mac, io, d.irq));
                }
            }
            _ => {}
        }
    }
    None
}

pub fn info() -> Option<CardInfo> {
    *INFO.lock()
}

pub fn has_events() -> bool {
    match KIND.load(Ordering::Acquire) {
        NE2000 => ne2000::has_events(),
        RTL8139 => rtl8139::has_events(),
        _ => false,
    }
}

/// 届いているフレームをすべて `f` に渡す
pub fn receive_all(f: impl FnMut(&[u8])) {
    match KIND.load(Ordering::Acquire) {
        NE2000 => ne2000::receive_all(f),
        RTL8139 => rtl8139::receive_all(f),
        _ => {}
    }
}

pub fn tx_idle() -> bool {
    match KIND.load(Ordering::Acquire) {
        NE2000 => ne2000::tx_idle(),
        RTL8139 => rtl8139::tx_idle(),
        _ => true,
    }
}

/// 送る（tx_idle を確かめてから呼ぶ）
pub fn send(frame: &[u8]) -> bool {
    match KIND.load(Ordering::Acquire) {
        NE2000 => ne2000::send(frame),
        RTL8139 => rtl8139::send(frame),
        _ => false,
    }
}

pub fn tx_timeout() {
    match KIND.load(Ordering::Acquire) {
        NE2000 => ne2000::tx_timeout(),
        RTL8139 => rtl8139::tx_timeout(),
        _ => {}
    }
}

/// TCP で知らせる受信の窓（カードの受信バッファにあふれずに収まる量）
pub fn recv_window() -> u32 {
    match KIND.load(Ordering::Acquire) {
        RTL8139 => rtl8139::RECV_WINDOW,
        _ => ne2000::RECV_WINDOW,
    }
}
