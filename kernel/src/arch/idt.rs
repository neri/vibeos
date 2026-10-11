//! IDT と割り込みの入口（全ベクタ共通のスタブ）
//!
//! 各ベクタのスタブはエラーコード（無ければ 0）とベクタ番号を積んで `isr_common` へ飛ぶ。
//! `isr_common` は全レジスタを積んで `TrapFrame` を作り、`trap::dispatch` を呼ぶ。

use core::arch::{asm, global_asm};
use core::mem::size_of;

use super::gdt::KERNEL_CODE;
use crate::sync::BootCell;

pub const IRQ_BASE: u8 = 0x20;
pub const SYSCALL_VECTOR: u8 = 0x80;
const STUB_COUNT: usize = 48;

global_asm!(
    // CPU がエラーコードを積まない例外
    ".macro ISR_NOERR n",
    ".global isr_stub_\\n",
    "isr_stub_\\n:",
    "    push 0",
    "    push \\n",
    "    jmp isr_common",
    ".endm",
    // CPU がエラーコードを積む例外
    ".macro ISR_ERR n",
    ".global isr_stub_\\n",
    "isr_stub_\\n:",
    "    push \\n",
    "    jmp isr_common",
    ".endm",
    ".section .text",
    ".irp n, 0,1,2,3,4,5,6,7,9,15,16,18,19,20,22,23,24,25,26,27,28,31",
    "ISR_NOERR \\n",
    ".endr",
    ".irp n, 8,10,11,12,13,14,17,21,29,30",
    "ISR_ERR \\n",
    ".endr",
    ".irp n, 32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,128",
    "ISR_NOERR \\n",
    ".endr",
    "isr_common:",
    "    pushad",
    // セグメントレジスタは 32bit で積む（Intel 記法の `push ds` は 16bit の push になってしまうので EAX 経由）
    "    mov eax, ds",
    "    push eax",
    "    mov eax, es",
    "    push eax",
    "    mov eax, fs",
    "    push eax",
    "    mov eax, gs",
    "    push eax",
    "    mov ax, {kdata}",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    cld",
    // 割り込みはどこで起きるか分からないので、ESP を 16 の倍数にそろえてから呼ぶ（データレイアウトの S128）。
    // EBX は呼び出し先が保存するレジスタなので、TrapFrame の番地をそこに取っておく
    "    mov ebx, esp",
    "    and esp, -16",
    "    sub esp, 12",
    "    push ebx",
    "    call {dispatch}",
    "    mov esp, ebx",
    ".global isr_return",
    "isr_return:",
    "    pop eax",
    "    mov gs, ax",
    "    pop eax",
    "    mov fs, ax",
    "    pop eax",
    "    mov es, ax",
    "    pop eax",
    "    mov ds, ax",
    "    popad",
    "    add esp, 8",
    "    iretd",
    // スタブの番地の表（0〜47）
    ".section .rodata",
    ".balign 4",
    ".global isr_stub_table",
    "isr_stub_table:",
    ".irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47",
    ".long isr_stub_\\n",
    ".endr",
    ".section .text",
    kdata = const super::gdt::KERNEL_DATA as u32,
    dispatch = sym super::trap::dispatch,
);

unsafe extern "C" {
    static isr_stub_table: [u32; STUB_COUNT];
    fn isr_stub_128();
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Gate {
    offset_lo: u16,
    selector: u16,
    zero: u8,
    type_attr: u8,
    offset_hi: u16,
}

const GATE_INTERRUPT_KERNEL: u8 = 0x8E; // present, DPL 0, 32bit 割り込みゲート
// システムコール用: present, DPL 3（int 0x80 をユーザから呼べる）, トラップゲート（割り込みを止めない。
// システムコールの中で待つことがあるため）
const GATE_TRAP_USER: u8 = 0xEF;
const GATE_TASK: u8 = 0x85; // present, DPL 0, タスクゲート
const DOUBLE_FAULT_VECTOR: usize = 8;

static IDT: BootCell<[Gate; 256]> =
    BootCell::new([Gate { offset_lo: 0, selector: 0, zero: 0, type_attr: 0, offset_hi: 0 }; 256]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u32,
}

fn gate(handler: u32, type_attr: u8) -> Gate {
    Gate {
        offset_lo: handler as u16,
        selector: KERNEL_CODE,
        zero: 0,
        type_attr,
        offset_hi: (handler >> 16) as u16,
    }
}

/// IDT を設定して読み込む。割り込み禁止中に 1 回だけ呼ぶこと
pub fn init() {
    // SAFETY: 初期化中で、まだ割り込みは起きない。スタブの表はリンク時に決まる
    unsafe {
        let idt = &mut *IDT.as_ptr();
        for (i, &stub) in isr_stub_table.iter().enumerate() {
            idt[i] = gate(stub, GATE_INTERRUPT_KERNEL);
        }
        idt[SYSCALL_VECTOR as usize] = gate(isr_stub_128 as *const () as u32, GATE_TRAP_USER);
        // 二重フォルトは別の TSS（別のスタック）に切り替えて処理する
        idt[DOUBLE_FAULT_VECTOR] = Gate {
            offset_lo: 0,
            selector: super::gdt::DOUBLE_FAULT_TSS,
            zero: 0,
            type_attr: GATE_TASK,
            offset_hi: 0,
        };
        let ptr = DescriptorPointer { limit: (size_of::<[Gate; 256]>() - 1) as u16, base: idt.as_ptr() as u32 };
        asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
    }
}
