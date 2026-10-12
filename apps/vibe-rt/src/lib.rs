//! ユーザアプリ用のランタイム
//!
//! アプリは次のように書く:
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use vibe_rt::println;
//! vibe_rt::entry!(main);
//! fn main() -> i32 {
//!     println!("hello");
//!     0
//! }
//! ```
//! アプリの build.rs でリンカスクリプト（targets/app.ld）を指定すること（apps/hello/build.rs を参照）。

#![no_std]

extern crate alloc;

pub mod env;
pub mod gfx;
pub mod io;
pub mod net;
pub mod process;
pub mod sys;

use core::fmt::{self, Write};

pub use vibe_abi as abi;
pub use vibe_time as time;
pub use vibe_font as font;
pub use vibe_gfx as gfx_lib;

// 入口: カーネルはスタックに [argc][argv...][NULL][envp...][NULL] を置いて ring 3 の _start に飛んでくる
core::arch::global_asm!(
    ".section .text.entry, \"ax\"",
    ".global _start",
    "_start:",
    "    xor ebp, ebp",
    "    mov eax, [esp]",
    "    lea ebx, [esp + 4]",
    "    lea ecx, [ebx + eax * 4 + 4]",
    "    and esp, -16",
    "    sub esp, 4",
    "    push ecx",
    "    push ebx",
    "    push eax",
    "    call {start}",
    "    ud2",
    start = sym rt_start,
);

static mut ARGS: (usize, *const *const u8) = (0, core::ptr::null());

unsafe extern "Rust" {
    fn __vibe_main() -> i32;
}

extern "C" fn rt_start(argc: usize, argv: *const *const u8, envp: *const *const u8) -> ! {
    // SAFETY: 始まってすぐ、ほかに誰も触っていない
    unsafe { ARGS = (argc, argv) };
    // SAFETY: カーネルが 0 で終わる文字列へのポインタの配列（NULL で終わる）を置いた
    unsafe { env::init(envp) };
    // SAFETY: entry! マクロが定義する
    let code = unsafe { __vibe_main() };
    sys::exit(code)
}

/// アプリの main を指定する
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        fn __vibe_main() -> i32 {
            $main()
        }
    };
}

/// コマンドライン引数（0 番目はプログラムの名前）
pub fn args() -> impl Iterator<Item = &'static str> {
    // SAFETY: rt_start で一度だけ設定し、以降は読むだけ
    let (argc, argv) = unsafe { ARGS };
    // SAFETY: カーネルが 0 で終わる文字列へのポインタの配列を置いた
    (0..argc).map(move |i| unsafe { c_str(*argv.add(i)) })
}

/// 0 で終わる文字列（UTF-8 でなければ空）
///
/// # Safety
/// `p` は 0 で終わる、プロセスが終わるまで変わらない文字列を指すこと
unsafe fn c_str(p: *const u8) -> &'static str {
    let mut len = 0;
    // SAFETY: 呼ぶ側の約束
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        core::str::from_utf8(core::slice::from_raw_parts(p, len)).unwrap_or("")
    }
}

/// fd が端末につながっているか
pub fn isatty(fd: i32) -> bool {
    sys::fd_kind(fd) == abi::FD_CONSOLE as i32
}

pub struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        sys::write(abi::STDOUT, s.as_bytes());
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = Stdout.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::_print(format_args!("{}\n", format_args!($($arg)*))) };
}

pub struct Stderr;

impl Write for Stderr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        sys::write(abi::STDERR, s.as_bytes());
        Ok(())
    }
}

#[doc(hidden)]
pub fn _eprint(args: fmt::Arguments) {
    let _ = Stderr.write_fmt(args);
}

/// 標準エラーに書く
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::_eprint(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint!("\n") };
    ($($arg:tt)*) => { $crate::_eprint(format_args!("{}\n", format_args!($($arg)*))) };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\npanic: {}", info);
    sys::exit(101)
}

// --- ヒープ（sbrk で広げる） -------------------------------------------------

struct UserHeap {
    heap: core::cell::UnsafeCell<vibe_alloc::heap::Heap>,
}

// SAFETY: アプリは 1 つのスレッドで動く
unsafe impl Sync for UserHeap {}

#[global_allocator]
static HEAP: UserHeap = UserHeap { heap: core::cell::UnsafeCell::new(vibe_alloc::heap::Heap::empty()) };

const GROW_MIN: usize = 64 * 1024;

unsafe impl core::alloc::GlobalAlloc for UserHeap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // SAFETY: 1 スレッドなので同時に触られない
        let heap = unsafe { &mut *self.heap.get() };
        let p = heap.alloc(layout);
        if !p.is_null() {
            return p;
        }
        let grow = (layout.size() + layout.align() + 64).max(GROW_MIN).next_multiple_of(4096);
        let start = sys::sbrk(grow as i32);
        if start < 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: sbrk で得たばかりの領域
        unsafe { heap.add_region(start as usize, grow) };
        heap.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        // SAFETY: GlobalAlloc の約束
        unsafe { (*self.heap.get()).dealloc(ptr, layout) }
    }
}

/// 簡単な乱数（xorshift）。種は起動からの時間など
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// 0..n
    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }
}

/// ファイル全体を読む
pub fn read_file(path: &str) -> Result<alloc::vec::Vec<u8>, i32> {
    let fd = sys::open(path);
    if fd < 0 {
        return Err(-fd);
    }
    let r = io::read_to_end(fd);
    sys::close(fd);
    r
}

/// 何かキーが押されるまで待つ（離したイベントは読み飛ばす）
pub fn wait_key() -> abi::KeyEvent {
    loop {
        if let Some(k) = sys::getkey(false) {
            if k.pressed() {
                return k;
            }
        }
    }
}

/// 固定長の文字列に書式付きで書く（ヒープを使わない。あふれた分は捨てる）
pub fn format_fixed<const N: usize>(args: fmt::Arguments) -> FixedStr<N> {
    let mut s = FixedStr { buf: [0; N], len: 0 };
    let _ = s.write_fmt(args);
    s
}

pub struct FixedStr<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> core::ops::Deref for FixedStr<N> {
    type Target = str;
    fn deref(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for FixedStr<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(N - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}
