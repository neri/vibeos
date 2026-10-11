//! ring 3（ユーザモード）へ入る

use core::arch::asm;

use super::gdt::{USER_CODE, USER_DATA};

const EFLAGS_IF: u32 = 1 << 9;
const EFLAGS_RESERVED: u32 = 1 << 1;

/// `entry` から ring 3 で実行を始める（スタックは `user_esp`）。戻らない。
///
/// # Safety
/// 今のアドレス空間に、ユーザが実行できるコードとスタックが用意されていること。
/// TSS の esp0 がこのタスクのカーネルスタックになっていること（スケジューラが設定する）。
pub unsafe fn enter(entry: u32, user_esp: u32) -> ! {
    // SAFETY: iret で特権レベルを下げる。割り込みは iret で EFLAGS.IF が戻るまで止めておく
    unsafe {
        asm!(
            "cli",
            "mov ds, {ds:x}",
            "mov es, {ds:x}",
            "mov fs, {ds:x}",
            "mov gs, {ds:x}",
            "push {ss}",
            "push {esp}",
            "push {eflags}",
            "push {cs}",
            "push {eip}",
            "iretd",
            ds = in(reg) USER_DATA as u32,
            ss = const USER_DATA as u32,
            esp = in(reg) user_esp,
            eflags = const EFLAGS_IF | EFLAGS_RESERVED,
            cs = const USER_CODE as u32,
            eip = in(reg) entry,
            options(noreturn),
        );
    }
}
