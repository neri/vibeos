//! 割り込み・例外の振り分け
//!
//! 割り込みハンドラは最小限にする（DESIGN.md §6.4）: データを積んで、待っている人を起こすだけ。
//! 中でメモリを確保したり、何かを待ったりしない。

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::idt::{IRQ_BASE, SYSCALL_VECTOR};
use super::pic;

/// 割り込みの入口で積んだ全レジスタ（積んだ順の逆に並ぶ）
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    pub gs: u32,
    pub fs: u32,
    pub es: u32,
    pub ds: u32,
    pub edi: u32,
    pub esi: u32,
    pub ebp: u32,
    pub esp_at_pushad: u32,
    pub ebx: u32,
    pub edx: u32,
    pub ecx: u32,
    pub eax: u32,
    pub vector: u32,
    pub error: u32,
    pub eip: u32,
    pub cs: u32,
    pub eflags: u32,
    /// ring 3 から来たときだけ CPU が積む
    pub user_esp: u32,
    pub user_ss: u32,
}

const EXCEPTION_NAMES: [&str; 32] = [
    "Divide error (#DE)",
    "Debug (#DB)",
    "NMI",
    "Breakpoint (#BP)",
    "Overflow (#OF)",
    "Bound range (#BR)",
    "Invalid opcode (#UD)",
    "Device not available (#NM)",
    "Double fault (#DF)",
    "Coprocessor segment overrun",
    "Invalid TSS (#TS)",
    "Segment not present (#NP)",
    "Stack fault (#SS)",
    "General protection (#GP)",
    "Page fault (#PF)",
    "Reserved",
    "x87 FPU error (#MF)",
    "Alignment check (#AC)",
    "Machine check (#MC)",
    "SIMD error (#XM)",
    "Virtualization (#VE)",
    "Control protection (#CP)",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Hypervisor injection",
    "VMM communication",
    "Security exception",
    "Reserved",
];

const NO_HANDLER: usize = 0;
static IRQ_HANDLERS: [AtomicUsize; 16] = [const { AtomicUsize::new(NO_HANDLER) }; 16];

/// IRQ のハンドラを登録して、その IRQ を有効にする
pub fn set_irq_handler(irq: u8, handler: fn()) {
    IRQ_HANDLERS[irq as usize].store(handler as usize, Ordering::Release);
    pic::unmask(irq);
}

/// `isr_common` から呼ばれる
pub extern "C" fn dispatch(frame: &mut TrapFrame) {
    let vector = frame.vector as u8;
    match vector {
        0..=31 => exception(frame),
        v if (IRQ_BASE..IRQ_BASE + 16).contains(&v) => irq(v - IRQ_BASE),
        SYSCALL_VECTOR => {
            klog!("syscall {} (not implemented)", frame.eax);
            frame.eax = u32::MAX;
        }
        _ => klog!("unexpected interrupt {}", vector),
    }
}

fn irq(irq: u8) {
    if pic::is_spurious(irq) {
        return;
    }
    // ハンドラの中でタスクが切り替わっても次の割り込みが止まらないよう、先に EOI を送る
    // （割り込みゲートなので、ハンドラの実行中は割り込み禁止のまま）
    pic::end_of_interrupt(irq);
    let h = IRQ_HANDLERS[irq as usize].load(Ordering::Acquire);
    if h == NO_HANDLER {
        klog!("IRQ {} without handler", irq);
        return;
    }
    // SAFETY: set_irq_handler で fn() として登録した値
    let handler: fn() = unsafe { core::mem::transmute::<usize, fn()>(h) };
    handler();
}

fn exception(f: &TrapFrame) -> ! {
    let name = EXCEPTION_NAMES[f.vector as usize];
    let cr2 = if f.vector == 14 {
        let v: u32;
        // SAFETY: CR2 を読むだけ
        unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) };
        v
    } else {
        0
    };
    panic!(
        "{name}\nerr={:x} cr2={:08x}\neip={:08x} cs={:x} efl={:08x}\neax={:08x} ebx={:08x}\necx={:08x} edx={:08x}\nesi={:08x} edi={:08x}\nebp={:08x} esp={:08x}",
        f.error,
        cr2,
        f.eip,
        f.cs,
        f.eflags,
        f.eax,
        f.ebx,
        f.ecx,
        f.edx,
        f.esi,
        f.edi,
        f.ebp,
        // pushad が保存した ESP はベクタ番号の位置を指す。その上のベクタ・エラーコード・EIP・CS・EFLAGS を除くと
        // 割り込み直前の ESP（カーネル内で起きた場合）
        f.esp_at_pushad + 20,
    );
}

/// 二重フォルトのタスクゲートの入口（専用の TSS とスタックで動く）
pub extern "C" fn double_fault_entry() -> ! {
    panic!("Double fault (#DF)\nkernel stack overflow?");
}
