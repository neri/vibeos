//! ページング（4KB ページ, 2 段のページテーブル）
//!
//! カーネル空間（0〜1GB）は物理アドレスと同じ番地に対応付ける。対応付けるのは RAM の最後まで。
//! 0 番地のページは対応付けない（ヌルポインタの参照をページフォルトで捕まえるため）。
//!
//! カーネル空間のページテーブルは起動時に（RAM の最後まで）すべて作り、以降は増やさない。
//! プロセスごとのページディレクトリはカーネル空間の部分をコピーして共有するので、
//! 後からカーネル空間にページテーブルを足すと既存のプロセスに反映されないため。

use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use vibe_alloc::frame::PAGE_SIZE;

use super::pmm;

pub const PRESENT: u32 = 1 << 0;
pub const WRITABLE: u32 = 1 << 1;
#[allow(dead_code)]
pub const USER: u32 = 1 << 2;

/// カーネル空間の終わり（ここから上はユーザ空間）
pub const KERNEL_SPACE_END: usize = 0x4000_0000;
const ENTRIES: usize = 1024;
const TABLE_SPAN: usize = ENTRIES * PAGE_SIZE; // 1 つのページテーブルで 4MB

const CR0_WP: u32 = 1 << 16;
const CR0_PG: u32 = 1 << 31;

/// カーネルのページディレクトリ（物理アドレス）
static KERNEL_PD: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    let ram_end = pmm::ram_end().min(KERNEL_SPACE_END);
    let pd = pmm::alloc_zeroed_page().expect("paging: no memory for page directory");
    let mut addr = 0;
    while addr < ram_end {
        let pt = pmm::alloc_zeroed_page().expect("paging: no memory for page table");
        let table = pt as *mut u32;
        for i in 0..ENTRIES {
            let page = addr + i * PAGE_SIZE;
            if page != 0 && page < ram_end {
                // SAFETY: 割り当てたばかりのページテーブル
                unsafe { table.add(i).write(page as u32 | PRESENT | WRITABLE) };
            }
        }
        // SAFETY: 同上
        unsafe { (pd as *mut u32).add(addr / TABLE_SPAN).write(pt as u32 | PRESENT | WRITABLE) };
        addr += TABLE_SPAN;
    }
    KERNEL_PD.store(pd, Ordering::Relaxed);

    // SAFETY: 実行中のコード・データ・スタックはすべて同じ番地に対応付けたので、有効にしても続けて動ける
    unsafe {
        asm!(
            "mov cr3, {pd}",
            "mov {tmp}, cr0",
            "or {tmp}, {flags}",
            "mov cr0, {tmp}",
            pd = in(reg) pd,
            tmp = out(reg) _,
            flags = const CR0_PG | CR0_WP,
            options(nostack),
        );
    }
    klog!("paging: enabled, identity mapped up to {:#x}", ram_end);
}

#[allow(dead_code)]
pub fn kernel_page_directory() -> usize {
    KERNEL_PD.load(Ordering::Relaxed)
}

/// ページディレクトリ `pd` の中で `virt` を表すページテーブルのエントリへのポインタ。
/// ページテーブルが無ければ `create` のとき作る。
fn entry(pd: usize, virt: usize, create: bool) -> Option<*mut u32> {
    let pde = (pd as *mut u32).wrapping_add(virt >> 22);
    // SAFETY: pd はページディレクトリ（物理 = 仮想の番地で触れる）
    let mut pt = unsafe { pde.read() };
    if pt & PRESENT == 0 {
        // カーネル空間のページテーブルは起動時に作ったものだけを使う（モジュールの説明参照）
        if !create || virt < KERNEL_SPACE_END {
            return None;
        }
        let new = pmm::alloc_zeroed_page()? as u32;
        let flags = PRESENT | WRITABLE | USER;
        // SAFETY: 同上
        unsafe { pde.write(new | flags) };
        pt = new | flags;
    }
    let table = (pt & !0xFFF) as *mut u32;
    Some(table.wrapping_add((virt >> 12) & 0x3FF))
}

/// `virt` のページを `phys` に対応付ける（`flags` に PRESENT は自動で付く）
#[allow(dead_code)]
pub fn map(pd: usize, virt: usize, phys: usize, flags: u32) -> Result<(), &'static str> {
    let e = entry(pd, virt, true).ok_or("paging: no page table (out of memory, or kernel space beyond RAM)")?;
    // SAFETY: entry が返したのはページテーブルの中のエントリ
    unsafe { e.write((phys & !0xFFF) as u32 | flags | PRESENT) };
    flush(virt);
    Ok(())
}

/// `virt` のページの対応付けを外し、対応していた物理アドレスを返す
#[allow(dead_code)]
pub fn unmap(pd: usize, virt: usize) -> Option<usize> {
    let e = entry(pd, virt, false)?;
    // SAFETY: 同上
    let old = unsafe { e.read() };
    if old & PRESENT == 0 {
        return None;
    }
    // SAFETY: 同上
    unsafe { e.write(0) };
    flush(virt);
    Some((old & !0xFFF) as usize)
}

/// `virt` が対応付けられている物理アドレス
#[allow(dead_code)]
pub fn translate(pd: usize, virt: usize) -> Option<usize> {
    let e = entry(pd, virt, false)?;
    // SAFETY: 同上
    let v = unsafe { e.read() };
    (v & PRESENT != 0).then(|| (v & !0xFFF) as usize | (virt & 0xFFF))
}

fn flush(virt: usize) {
    // SAFETY: TLB の 1 ページ分を無効にするだけ
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}
