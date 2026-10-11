//! プロセスのアドレス空間（ページディレクトリ 1 つ分）
//!
//! カーネル空間（0〜1GB）の部分はカーネルのページディレクトリをコピーして共有する
//! （カーネル空間のページテーブルは起動時に作ったものから増えないので、コピーで十分）。
//! ユーザ空間（1GB〜3GB）のページとページテーブルはこのアドレス空間が持ち、捨てるときに解放する。

use core::arch::asm;

use vibe_abi::{USER_BASE, USER_END};
use vibe_alloc::frame::PAGE_SIZE;

use super::paging::{self, KERNEL_SPACE_END, PRESENT, USER, WRITABLE};
use super::pmm;

const PDE_COUNT: usize = 1024;
const KERNEL_PDES: usize = KERNEL_SPACE_END >> 22;

pub struct AddressSpace {
    /// ページディレクトリの物理アドレス
    pd: usize,
    /// プログラムの終わり（sbrk で動かす）
    brk: u32,
    /// sbrk で広げられる上限（スタックの下）
    brk_limit: u32,
}

impl AddressSpace {
    pub fn new() -> Option<AddressSpace> {
        let pd = pmm::alloc_zeroed_page()?;
        let kernel = paging::kernel_page_directory();
        // SAFETY: どちらもページディレクトリで、カーネル空間では物理アドレスと同じ番地で触れる
        unsafe { core::ptr::copy_nonoverlapping(kernel as *const u32, pd as *mut u32, KERNEL_PDES) };
        Some(AddressSpace { pd, brk: USER_BASE, brk_limit: USER_BASE })
    }

    pub fn page_directory(&self) -> usize {
        self.pd
    }

    /// ユーザ空間の `virt` のページを用意して物理アドレスを返す（既にあればそれを返す）。
    /// 新しいページは 0 で埋める。`writable` なら書き込み可にする（既存のページも書き込み可に変える）
    pub fn map_user_page(&mut self, virt: u32, writable: bool) -> Option<usize> {
        let virt = virt & !(PAGE_SIZE as u32 - 1);
        if !(USER_BASE..USER_END).contains(&virt) {
            return None;
        }
        let flags = USER | if writable { WRITABLE } else { 0 };
        if let Some((phys, old)) = paging::lookup(self.pd, virt as usize) {
            if writable && old & WRITABLE == 0 {
                paging::map(self.pd, virt as usize, phys, flags).ok()?;
            }
            return Some(phys);
        }
        let phys = pmm::alloc_zeroed_page()?;
        if paging::map(self.pd, virt as usize, phys, flags).is_err() {
            pmm::free_pages(phys, 1);
            return None;
        }
        Some(phys)
    }

    /// プログラムを読み込んだ後に、ヒープの始まりと上限を決める
    pub fn set_brk(&mut self, start: u32, limit: u32) {
        self.brk = start;
        self.brk_limit = limit;
    }

    /// ヒープを `increment` バイト広げ、広げる前の終わりを返す
    pub fn sbrk(&mut self, increment: i32) -> Option<u32> {
        let old = self.brk;
        if increment <= 0 {
            return Some(old); // 縮めるのは未対応（何もしない）
        }
        let new = old.checked_add(increment as u32).filter(|&n| n <= self.brk_limit)?;
        let mut page = old.next_multiple_of(PAGE_SIZE as u32);
        // old がページの途中なら、そのページは既にある
        if old % PAGE_SIZE as u32 != 0 {
            page = old & !(PAGE_SIZE as u32 - 1);
        }
        while page < new {
            self.map_user_page(page, true)?;
            page += PAGE_SIZE as u32;
        }
        self.brk = new;
        Some(old)
    }

    /// [addr, addr + len) がすべてユーザが読める（`write` なら書ける）ページか
    pub fn check_user(&self, addr: u32, len: u32, write: bool) -> bool {
        let Some(end) = addr.checked_add(len) else { return false };
        if addr < USER_BASE || end > USER_END {
            return false;
        }
        let mut page = addr & !(PAGE_SIZE as u32 - 1);
        while page < end {
            match paging::lookup(self.pd, page as usize) {
                Some((_, flags)) if flags & USER != 0 && (!write || flags & WRITABLE != 0) => {}
                _ => return false,
            }
            page += PAGE_SIZE as u32;
        }
        true
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        // 使用中のページディレクトリは捨てられない（呼び出し側がカーネルのものに切り替えてから捨てる）
        let cr3: usize;
        // SAFETY: CR3 を読むだけ
        unsafe { asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags)) };
        assert_ne!(cr3, self.pd, "dropping the active address space");
        let pd = self.pd as *const u32;
        for i in KERNEL_PDES..PDE_COUNT {
            // SAFETY: ページディレクトリとページテーブルは物理 = 仮想の番地で触れる
            let pde = unsafe { pd.add(i).read() };
            if pde & PRESENT == 0 {
                continue;
            }
            let pt = (pde & !0xFFF) as usize;
            for j in 0..1024 {
                // SAFETY: 同上
                let pte = unsafe { (pt as *const u32).add(j).read() };
                if pte & PRESENT != 0 {
                    pmm::free_pages((pte & !0xFFF) as usize, 1);
                }
            }
            pmm::free_pages(pt, 1);
        }
        pmm::free_pages(self.pd, 1);
    }
}
