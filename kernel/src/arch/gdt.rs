//! GDT と TSS
//!
//! | セレクタ | 内容 |
//! |---|---|
//! | 0x08 | カーネルのコード |
//! | 0x10 | カーネルのデータ |
//! | 0x1B | ユーザのコード（RPL 3） |
//! | 0x23 | ユーザのデータ（RPL 3） |
//! | 0x28 | TSS |

use core::arch::asm;
use core::mem::size_of;

use crate::sync::BootCell;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
#[allow(dead_code)]
pub const USER_CODE: u16 = 0x18 | 3;
#[allow(dead_code)]
pub const USER_DATA: u16 = 0x20 | 3;
const TSS_SELECTOR: u16 = 0x28;

#[repr(C, packed)]
pub struct Tss {
    link: u32,
    esp0: u32,
    ss0: u32,
    unused: [u32; 22],
    trap: u16,
    iomap_base: u16,
}

const _: () = assert!(size_of::<Tss>() == 104);

static TSS: BootCell<Tss> =
    BootCell::new(Tss { link: 0, esp0: 0, ss0: 0, unused: [0; 22], trap: 0, iomap_base: 0 });

static GDT: BootCell<[u64; 6]> = BootCell::new([0; 6]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u32,
}

const fn segment(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    (limit & 0xFFFF) as u64
        | ((base & 0xFF_FFFF) as u64) << 16
        | (access as u64) << 40
        | (((limit >> 16) & 0xF) as u64) << 48
        | ((flags & 0xF) as u64) << 52
        | ((base >> 24) as u64) << 56
}

/// GDT と TSS を設定して読み込む。割り込み禁止中に 1 回だけ呼ぶこと
pub fn init(kernel_stack_top: u32) {
    let tss = TSS.as_ptr();
    // SAFETY: 初期化中で、まだ誰も GDT/TSS を参照していない
    unsafe {
        (*tss).ss0 = KERNEL_DATA as u32;
        (*tss).esp0 = kernel_stack_top;
        (*tss).iomap_base = size_of::<Tss>() as u16; // I/O 許可ビットマップなし

        let gdt = &mut *GDT.as_ptr();
        gdt[0] = 0;
        gdt[1] = segment(0, 0xF_FFFF, 0x9A, 0xC); // カーネル コード
        gdt[2] = segment(0, 0xF_FFFF, 0x92, 0xC); // カーネル データ
        gdt[3] = segment(0, 0xF_FFFF, 0xFA, 0xC); // ユーザ コード
        gdt[4] = segment(0, 0xF_FFFF, 0xF2, 0xC); // ユーザ データ
        gdt[5] = segment(tss as u32, size_of::<Tss>() as u32 - 1, 0x89, 0x0); // TSS

        let ptr = DescriptorPointer { limit: (size_of::<[u64; 6]>() - 1) as u16, base: gdt.as_ptr() as u32 };
        asm!(
            "lgdt [{ptr}]",
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov fs, {data:x}",
            "mov gs, {data:x}",
            "mov ss, {data:x}",
            "push {code}",
            "lea {tmp}, [2f]",
            "push {tmp}",
            "retf",
            "2:",
            "ltr {tss:x}",
            ptr = in(reg) &ptr,
            data = in(reg) KERNEL_DATA as u32,
            code = const KERNEL_CODE as u32,
            tss = in(reg) TSS_SELECTOR as u32,
            tmp = out(reg) _,
        );
    }
}

/// ring 3 から割り込みで戻ってきたときに使うカーネルスタックを設定する
#[allow(dead_code)]
pub fn set_kernel_stack(top: u32) {
    // SAFETY: TSS.esp0 は CPU が特権レベルの切り替え時に読むだけ。書き換えは割り込み禁止中に行う
    unsafe { (*TSS.as_ptr()).esp0 = top };
}
