//! CPU の制御命令

use core::arch::asm;

const EFLAGS_IF: u32 = 1 << 9;

pub fn interrupts_enabled() -> bool {
    let flags: u32;
    // SAFETY: EFLAGS を読むだけ
    unsafe { asm!("pushfd", "pop {}", out(reg) flags, options(nomem, preserves_flags)) }
    flags & EFLAGS_IF != 0
}

/// 割り込みを禁止する
pub fn disable_interrupts() {
    // SAFETY: 割り込み禁止は常に安全（解除し忘れはロックのガードが面倒を見る）
    unsafe { asm!("cli", options(nomem, nostack)) }
}

/// 割り込みを許可する
///
/// # Safety
/// IDT が用意されていて、割り込まれても壊れない状態であること。
pub unsafe fn enable_interrupts() {
    // SAFETY: 呼び出し側の責任
    unsafe { asm!("sti", options(nomem, nostack)) }
}

/// 次の割り込みまで止まる
pub fn halt() {
    // SAFETY: hlt は割り込みが来れば戻る。割り込み禁止中なら永久に止まるが、それは呼び出し側の意図
    unsafe { asm!("hlt", options(nomem, nostack, preserves_flags)) }
}

/// 割り込みを禁止して永久に止まる
pub fn halt_forever() -> ! {
    loop {
        disable_interrupts();
        halt();
    }
}

const EFLAGS_ID: u32 = 1 << 21;

pub struct CpuInfo {
    pub vendor: [u8; 12],
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
}

impl CpuInfo {
    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("?")
    }
}

/// CPUID 命令が使えるか（EFLAGS の ID ビットを書き換えられるか）
fn cpuid_supported() -> bool {
    let (before, after): (u32, u32);
    // SAFETY: EFLAGS の ID ビットを反転して読み戻し、元に戻す
    unsafe {
        asm!(
            "pushfd",
            "pop {before}",
            "mov {after}, {before}",
            "xor {after}, {id}",
            "push {after}",
            "popfd",
            "pushfd",
            "pop {after}",
            "push {before}",
            "popfd",
            before = out(reg) before,
            after = out(reg) after,
            id = const EFLAGS_ID,
        );
    }
    (before ^ after) & EFLAGS_ID != 0
}

fn cpuid(leaf: u32) -> [u32; 4] {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    // SAFETY: CPUID は読むだけ。EBX は LLVM が使うことがあるので退避して取り出す
    unsafe {
        asm!(
            "mov {tmp}, ebx",
            "cpuid",
            "xchg {tmp}, ebx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") 0 => ecx,
            out("edx") edx,
            options(nomem, nostack, preserves_flags),
        );
    }
    [eax, ebx, ecx, edx]
}

/// CPU の情報。CPUID が使えなければ None（i486 以前）
pub fn identify() -> Option<CpuInfo> {
    if !cpuid_supported() {
        return None;
    }
    let [max_leaf, b, c, d] = cpuid(0);
    let mut vendor = [0u8; 12];
    vendor[0..4].copy_from_slice(&b.to_le_bytes());
    vendor[4..8].copy_from_slice(&d.to_le_bytes());
    vendor[8..12].copy_from_slice(&c.to_le_bytes());
    let (mut family, mut model, mut stepping) = (4, 0, 0);
    if max_leaf >= 1 {
        let [a, ..] = cpuid(1);
        stepping = a & 0xF;
        model = (a >> 4) & 0xF;
        family = (a >> 8) & 0xF;
        if family == 0xF {
            family += (a >> 20) & 0xFF;
        }
        if family >= 6 {
            model += ((a >> 16) & 0xF) << 4;
        }
    }
    Some(CpuInfo { vendor, family, model, stepping })
}
