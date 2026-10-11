//! GDT と TSS
//!
//! | セレクタ | 内容 |
//! |---|---|
//! | 0x08 | カーネルのコード |
//! | 0x10 | カーネルのデータ |
//! | 0x1B | ユーザのコード（RPL 3） |
//! | 0x23 | ユーザのデータ（RPL 3） |
//! | 0x28 | TSS |
//! | 0x30 | 二重フォルト用の TSS（タスクゲートで別スタックに切り替える） |

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
pub const DOUBLE_FAULT_TSS: u16 = 0x30;
const GDT_ENTRIES: usize = 7;

#[repr(C, packed)]
pub struct Tss {
    link: u32,
    esp0: u32,
    ss0: u32,
    esp1: u32,
    ss1: u32,
    esp2: u32,
    ss2: u32,
    cr3: u32,
    eip: u32,
    eflags: u32,
    eax: u32,
    ecx: u32,
    edx: u32,
    ebx: u32,
    esp: u32,
    ebp: u32,
    esi: u32,
    edi: u32,
    es: u32,
    cs: u32,
    ss: u32,
    ds: u32,
    fs: u32,
    gs: u32,
    ldt: u32,
    trap: u16,
    iomap_base: u16,
}

const _: () = assert!(size_of::<Tss>() == 104);

const EMPTY_TSS: Tss = Tss {
    link: 0,
    esp0: 0,
    ss0: 0,
    esp1: 0,
    ss1: 0,
    esp2: 0,
    ss2: 0,
    cr3: 0,
    eip: 0,
    eflags: 0,
    eax: 0,
    ecx: 0,
    edx: 0,
    ebx: 0,
    esp: 0,
    ebp: 0,
    esi: 0,
    edi: 0,
    es: 0,
    cs: 0,
    ss: 0,
    ds: 0,
    fs: 0,
    gs: 0,
    ldt: 0,
    trap: 0,
    iomap_base: 0,
};

static TSS: BootCell<Tss> = BootCell::new(EMPTY_TSS);
static DOUBLE_FAULT_TSS_DATA: BootCell<Tss> = BootCell::new(EMPTY_TSS);

const DOUBLE_FAULT_STACK_SIZE: usize = 8192;
#[repr(C, align(16))]
struct DoubleFaultStack([u8; DOUBLE_FAULT_STACK_SIZE]);
static DOUBLE_FAULT_STACK: BootCell<DoubleFaultStack> = BootCell::new(DoubleFaultStack([0; DOUBLE_FAULT_STACK_SIZE]));

static GDT: BootCell<[u64; GDT_ENTRIES]> = BootCell::new([0; GDT_ENTRIES]);

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
        let df = DOUBLE_FAULT_TSS_DATA.as_ptr();
        gdt[6] = segment(df as u32, size_of::<Tss>() as u32 - 1, 0x89, 0x0); // 二重フォルト用 TSS

        let ptr =
            DescriptorPointer { limit: (size_of::<[u64; GDT_ENTRIES]>() - 1) as u16, base: gdt.as_ptr() as u32 };
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
pub fn set_kernel_stack(top: u32) {
    // SAFETY: TSS.esp0 は CPU が特権レベルの切り替え時に読むだけ。書き換えは割り込み禁止中に行う
    unsafe { (*TSS.as_ptr()).esp0 = top };
}

/// 二重フォルト用の TSS を設定する（ページングを有効にした後に、そのページディレクトリで呼ぶ）
///
/// 二重フォルトはカーネルスタックがあふれたとき（ガードページに触れて、例外の情報を積めない）に
/// 起きるので、別のスタックに切り替えて処理する必要がある。IDT の 8 番はこの TSS へのタスクゲート。
pub fn init_double_fault(page_directory: u32) {
    let df = DOUBLE_FAULT_TSS_DATA.as_ptr();
    // 関数の入口と同じ形（ESP+4 が 16 の倍数）にしておく
    let stack_top = DOUBLE_FAULT_STACK.as_ptr() as u32 + DOUBLE_FAULT_STACK_SIZE as u32 - 4;
    // SAFETY: 初期化中で、まだ二重フォルトは起きない
    unsafe {
        (*df).cr3 = page_directory;
        (*df).eip = super::trap::double_fault_entry as *const () as u32;
        (*df).eflags = 0x2; // 割り込み禁止
        (*df).esp = stack_top;
        (*df).esp0 = stack_top;
        (*df).ss0 = KERNEL_DATA as u32;
        (*df).cs = KERNEL_CODE as u32;
        (*df).ss = KERNEL_DATA as u32;
        (*df).ds = KERNEL_DATA as u32;
        (*df).es = KERNEL_DATA as u32;
        (*df).fs = KERNEL_DATA as u32;
        (*df).gs = KERNEL_DATA as u32;
        (*df).iomap_base = size_of::<Tss>() as u16;
    }
}
