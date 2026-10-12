//! 環境変数（DECISIONS.md D-040）
//!
//! 環境はカーネルが覚えておくものではなく、各プロセスのメモリの中にある。起動したときにスタックに
//! 積まれた `envp` から作り、`set_var`・`remove_var` で自分の分を変える。`spawn`・`Command` は
//! 今の環境をそのまま子に渡す（`Command::env_clear` などで変えられる）。

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

/// "名前=値" の並び
struct Env(UnsafeCell<Option<Vec<String>>>);

// SAFETY: アプリは 1 つのスレッドで動く
unsafe impl Sync for Env {}

static ENV: Env = Env(UnsafeCell::new(None));
static ENVP: Envp = Envp(UnsafeCell::new(core::ptr::null()));

struct Envp(UnsafeCell<*const *const u8>);

// SAFETY: 同上
unsafe impl Sync for Envp {}

/// 起動時に一度だけ呼ぶ（中身を読むのは最初に使ったとき。使わないアプリはヒープを使わない）
///
/// # Safety
/// `envp` は NULL で終わる、0 で終わる文字列へのポインタの配列（プロセスが終わるまで変わらない）
pub(crate) unsafe fn init(envp: *const *const u8) {
    // SAFETY: 始まってすぐ、ほかに誰も触っていない
    unsafe { *ENVP.0.get() = envp };
}

fn with<R>(f: impl FnOnce(&mut Vec<String>) -> R) -> R {
    // SAFETY: 1 スレッドで、この関数の外に参照を出さない
    let env = unsafe { &mut *ENV.0.get() };
    let list = env.get_or_insert_with(|| {
        let mut v = Vec::new();
        // SAFETY: init で渡されたもの
        let envp = unsafe { *ENVP.0.get() };
        if !envp.is_null() {
            let mut i = 0;
            // SAFETY: init の約束
            unsafe {
                while !(*envp.add(i)).is_null() {
                    v.push(String::from(crate::c_str(*envp.add(i))));
                    i += 1;
                }
            }
        }
        v
    });
    f(list)
}

fn split(entry: &str) -> (&str, &str) {
    entry.split_once('=').unwrap_or((entry, ""))
}

/// 環境変数の一覧（名前, 値）
pub fn vars() -> Vec<(String, String)> {
    with(|l| {
        l.iter()
            .map(|e| {
                let (k, v) = split(e);
                (String::from(k), String::from(v))
            })
            .collect()
    })
}

/// 環境変数の値
pub fn var(name: &str) -> Option<String> {
    with(|l| l.iter().map(|e| split(e)).find(|(k, _)| *k == name).map(|(_, v)| String::from(v)))
}

pub fn set_var(name: &str, value: &str) {
    with(|l| {
        let entry = alloc::format!("{name}={value}");
        match l.iter().position(|e| split(e).0 == name) {
            Some(i) => l[i] = entry,
            None => l.push(entry),
        }
    })
}

pub fn remove_var(name: &str) {
    with(|l| l.retain(|e| split(e).0 != name))
}

/// "名前=値" の並び（子に渡す形）
pub fn entries() -> Vec<String> {
    with(|l| l.clone())
}
