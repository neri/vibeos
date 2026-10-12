//! カーネル内テスト（qemu-test 機能のときだけ）
//!
//! 起動処理が終わった後に全テストを順に実行し、すべて通れば QEMU を終了コード 33 で終える。
//! 失敗はテストの中で panic させる（panic 処理が終了コード 35 で QEMU を終える）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::drivers::{pit, qemu};
use crate::fs;
use crate::mm::{heap, paging, pmm};
use crate::task::{self, sched, wait::WaitQueue};

const TESTS: &[(&str, fn())] = &[
    ("heap_vec", heap_vec),
    ("heap_string", heap_string),
    ("heap_btreemap", heap_btreemap),
    ("heap_grows", heap_grows),
    ("heap_reuses_freed_memory", heap_reuses_freed_memory),
    ("pmm_alloc_free", pmm_alloc_free),
    ("paging_map_unmap", paging_map_unmap),
    ("sched_preemption", sched_preemption),
    ("sched_wait_queue", sched_wait_queue),
    ("sched_sleep_accuracy", sched_sleep_accuracy),
    ("sched_idle_halts", sched_idle_halts),
    ("sched_switch_stress", sched_switch_stress),
    ("sched_reclaims_stacks", sched_reclaims_stacks),
    ("mutex_sleeps_while_contended", mutex_sleeps_while_contended),
    ("fs_kernel_file_matches_boot_info", fs_kernel_file_matches_boot_info),
    ("lzsa_asm_decodes_samples", lzsa_asm_decodes_samples),
    ("lzsa_asm_rejects_broken_data", lzsa_asm_rejects_broken_data),
    ("fs_read_text_files", fs_read_text_files),
    ("fs_errors", fs_errors),
    ("fs_fd_read_in_chunks", fs_fd_read_in_chunks),
    ("fs_cwd_is_per_task", fs_cwd_is_per_task),
    ("process_exit_codes", process_exit_codes),
    ("process_spawn_errors", process_spawn_errors),
    ("process_reclaims_memory", process_reclaims_memory),
    ("fdc_read_matches_ramdisk", fdc_read_matches_ramdisk),
    ("fs_write_and_sync", fs_write_and_sync),
];

pub fn run_all() -> ! {
    klog!("ktest: running {} tests", TESTS.len());
    for (name, test) in TESTS {
        klog!("ktest: {} ...", name);
        test();
        klog!("ktest: {} ok", name);
    }
    klog!("ktest: all passed");
    qemu::exit(qemu::EXIT_SUCCESS)
}

fn heap_vec() {
    let mut v = Vec::new();
    for i in 0..100_000u32 {
        v.push(i);
    }
    let sum: u64 = v.iter().map(|&x| x as u64).sum();
    assert_eq!(sum, 99_999 * 100_000 / 2);
}

fn heap_string() {
    let s = format!("{}-{}-{:x}", "vibe", 42, 255);
    assert_eq!(s, "vibe-42-ff");
    let mut t = String::new();
    for _ in 0..1000 {
        t.push_str("abc");
    }
    assert_eq!(t.len(), 3000);
}

fn heap_btreemap() {
    let mut m = BTreeMap::new();
    for i in 0..1000 {
        m.insert(i * 7 % 1000, i);
    }
    assert_eq!(m.len(), 1000);
    for i in 0..500 {
        m.remove(&i);
    }
    assert_eq!(m.first_key_value().map(|(k, _)| *k), Some(500));
}

fn heap_grows() {
    // 最初のヒープ（1MB）より大きい割り当てで、物理ページから追加される
    let (_, total_before, _) = heap::stats();
    let big: Box<[u8]> = vec![0xAB; 3 * 1024 * 1024].into_boxed_slice();
    assert!(big.iter().all(|&b| b == 0xAB));
    let (_, total_after, _) = heap::stats();
    assert!(total_after > total_before);
}

fn heap_reuses_freed_memory() {
    let (free_before, _, _) = heap::stats();
    {
        let blocks: Vec<Vec<u8>> = (0..200).map(|i| vec![i as u8; 100 + i * 13]).collect();
        assert_eq!(blocks[199][0], 199);
    }
    let (free_after, _, _) = heap::stats();
    assert_eq!(free_before, free_after);
}

fn pmm_alloc_free() {
    let (free_before, _) = pmm::stats();
    let a = pmm::alloc_pages(10).expect("alloc 10 pages");
    assert_eq!(a % 4096, 0);
    assert_eq!(pmm::stats().0, free_before - 10);
    pmm::free_pages(a, 10);
    assert_eq!(pmm::stats().0, free_before);
}

fn paging_map_unmap() {
    // ユーザ空間の番地に物理ページを対応付け、両方の番地から同じ中身が見えることを確かめる
    let pd = paging::kernel_page_directory();
    let virt = 0x5000_0000;
    let phys = pmm::alloc_zeroed_page().unwrap();
    paging::map(pd, virt, phys, paging::WRITABLE).unwrap();
    assert_eq!(paging::translate(pd, virt + 0x123), Some(phys + 0x123));
    // SAFETY: 今対応付けたページ
    unsafe {
        (virt as *mut u32).write_volatile(0xDEAD_BEEF);
        assert_eq!((phys as *const u32).read_volatile(), 0xDEAD_BEEF);
    }
    assert_eq!(paging::unmap(pd, virt), Some(phys));
    assert_eq!(paging::translate(pd, virt), None);
    pmm::free_pages(phys, 1);
    // カーネル空間で RAM の外にはページテーブルを作らない（0x3F000000 からは装置のメモリの窓なので避ける）
    assert!(paging::map(pd, 0x3000_0000, phys, paging::WRITABLE).is_err());
}

/// 譲らずに回り続けるスレッドが 3 つあっても、全員とこのタスクが進む
fn sched_preemption() {
    static STOP: AtomicBool = AtomicBool::new(false);
    static COUNTS: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
    let ids: Vec<_> = (0..3)
        .map(|i| {
            task::spawn("busy", move || {
                while !STOP.load(Ordering::Relaxed) {
                    COUNTS[i].fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    pit::sleep_ms(200);
    let counts: Vec<u32> = COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).collect();
    STOP.store(true, Ordering::Relaxed);
    for id in ids {
        task::join(id);
    }
    klog!("ktest: busy counters {:?}", counts);
    assert!(counts.iter().all(|&c| c > 0), "a busy thread never ran: {counts:?}");
}

/// 条件が成り立つまで眠り、notify_all で起きる
fn sched_wait_queue() {
    static WQ: WaitQueue = WaitQueue::new();
    static FLAG: AtomicBool = AtomicBool::new(false);
    static WOKE_AT: AtomicU32 = AtomicU32::new(0);
    let t0 = pit::ticks();
    let id = task::spawn("waiter", || {
        WQ.wait_until(|| FLAG.load(Ordering::Relaxed));
        WOKE_AT.store(pit::ticks(), Ordering::Relaxed);
    });
    pit::sleep_ms(100);
    assert_eq!(WOKE_AT.load(Ordering::Relaxed), 0, "waiter woke up before the flag was set");
    FLAG.store(true, Ordering::Relaxed);
    WQ.notify_all();
    task::join(id);
    let woke = WOKE_AT.load(Ordering::Relaxed).wrapping_sub(t0);
    assert!((10..=13).contains(&woke), "woke after {woke} ticks");
}

fn sched_sleep_accuracy() {
    let start = pit::ticks();
    pit::sleep_ms(100);
    let elapsed = pit::ticks().wrapping_sub(start);
    assert!((10..=12).contains(&elapsed), "slept {elapsed} ticks");
}

/// 誰も動かないとき、時間はアイドルタスク（hlt）で過ごす
fn sched_idle_halts() {
    let before = sched::idle_ticks();
    pit::sleep_ms(300);
    let idle = sched::idle_ticks() - before;
    assert!(idle >= 25, "idle only {idle} of 30 ticks");
}

/// たくさんのスレッドが頻繁に譲り合っても、数え漏れが無い
fn sched_switch_stress() {
    static TOTAL: AtomicU32 = AtomicU32::new(0);
    let ids: Vec<_> = (0..20)
        .map(|_| {
            task::spawn("stress", || {
                for _ in 0..500 {
                    TOTAL.fetch_add(1, Ordering::Relaxed);
                    task::yield_now();
                }
            })
        })
        .collect();
    for id in ids {
        task::join(id);
    }
    assert_eq!(TOTAL.load(Ordering::Relaxed), 20 * 500);
}

/// 終わったタスクのスタック（物理ページ）は回収される
fn sched_reclaims_stacks() {
    // ヒープが途中で物理ページを取りに行かないよう、先に余裕を持たせておく
    drop(vec![0u8; 256 * 1024]);
    // 前のテストで終わったタスクがまだ回収されていないことがあるので、先に回収させてから数える
    task::yield_now();
    let (free_before, _) = pmm::stats();
    let tasks_before = sched::tasks().len();
    for _ in 0..5 {
        let ids: Vec<_> = (0..20).map(|_| task::spawn("short", || task::yield_now())).collect();
        for id in ids {
            task::join(id);
        }
    }
    task::yield_now(); // 終わったタスクは次の切り替えで回収される
    let (free_after, _) = pmm::stats();
    assert_eq!(free_before, free_after, "stack pages were not returned");
    assert_eq!(sched::tasks().len(), tasks_before, "dead tasks remain");
}

/// 眠る Mutex: 持っている間に他のタスクが取りに来ても、持ち主は（割り込みを止めずに）進める
fn mutex_sleeps_while_contended() {
    use alloc::sync::Arc;
    let m = Arc::new(crate::sync::Mutex::new(0u32));
    let g = m.lock();
    let m2 = m.clone();
    let id = task::spawn("contender", move || {
        *m2.lock() += 1;
    });
    pit::sleep_ms(50); // 持ったまま眠っても、相手は待つだけで壊れない
    assert_eq!(*g, 0);
    drop(g);
    task::join(id);
    assert_eq!(*m.lock(), 1);
}

unsafe extern "C" {
    static __rodata_end: u8;
    /// ローダと同じ LZSA2 の展開ルーチン（boot/unlzsa2_test.asm）
    fn unlzsa2_call(src: *const u8, src_len: usize, dst: *mut u8, dst_cap: usize) -> isize;
}

/// ローダが置いたカーネル（.text・.rodata）が、KERNEL.SYS を Rust の展開器で展開したものと同じ。
/// 圧縮したカーネルなら、ローダのアセンブラの展開・BCJ・CRC32 を毎回 Rust の実装と突き合わせる
fn fs_kernel_file_matches_boot_info() {
    let info = crate::bootinfo::get();
    let file = fs::read_file("/kernel.sys").unwrap();
    let start = info.kernel_addr as usize;
    let len = &raw const __rodata_end as usize - start;
    // SAFETY: [kernel_addr, __rodata_end) はカーネルの .text・.rodata で、起動後に書き換えない
    let loaded = unsafe { core::slice::from_raw_parts(start as *const u8, len) };
    let image = if vibe_pack::is_packed(&file) {
        let mut out = vec![0; info.kernel_size as usize];
        assert_eq!(vibe_pack::unpack(&file, &mut out), Ok(out.len()));
        out
    } else {
        file
    };
    assert_eq!(image.len(), info.kernel_size as usize);
    assert!(image[..len] == *loaded, "kernel in memory differs from KERNEL.SYS");
}

/// アセンブラの展開ルーチンで展開する（`None` は壊れている）
fn asm_unlzsa2(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    // SAFETY: src・dst は有効なスライスで、展開ルーチンは dst の外に書かない
    let n = unsafe { unlzsa2_call(src.as_ptr(), src.len(), dst.as_mut_ptr(), dst.len()) };
    (n >= 0).then_some(n as usize)
}

/// build.rs が埋め込んだデータ: (元の大きさ, 元の CRC32, 圧縮したもの)
fn lzsa_samples() -> Vec<(usize, u32, &'static [u8])> {
    static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lzsa_samples.bin"));
    let u32_at = |i: usize| u32::from_le_bytes([BLOB[i], BLOB[i + 1], BLOB[i + 2], BLOB[i + 3]]);
    let mut v = Vec::new();
    let mut p = 4;
    for _ in 0..u32_at(0) {
        let (len, crc, n) = (u32_at(p) as usize, u32_at(p + 4), u32_at(p + 8) as usize);
        v.push((len, crc, &BLOB[p + 12..p + 12 + n]));
        p += 12 + n;
    }
    v
}

/// ホストで圧縮した境目のデータ（長さ・オフセットの形の境目、フレームの境目、遠いオフセット）を
/// ローダの展開ルーチンで展開し、元と同じになる
fn lzsa_asm_decodes_samples() {
    let samples = lzsa_samples();
    assert!(samples.len() > 300, "{}", samples.len());
    for (i, &(len, crc, packed)) in samples.iter().enumerate() {
        let mut out = vec![0u8; len];
        assert_eq!(asm_unlzsa2(packed, &mut out), Some(len), "sample {i}");
        assert_eq!(vibe_pack::crc32(&out), crc, "sample {i}");
    }
}

/// 壊れたデータ・途中で切れたデータ・出力先が足りないときに、出力の上限を越えて書かない
fn lzsa_asm_rejects_broken_data() {
    const GUARD: usize = 64;
    let check = |packed: &[u8], cap: usize| {
        let mut out = vec![0xA5u8; cap + GUARD];
        let r = asm_unlzsa2(packed, &mut out[..cap]);
        assert!(r.is_none_or(|n| n <= cap));
        assert!(out[cap..].iter().all(|&b| b == 0xA5), "wrote past the limit");
    };
    let mut seed = 1u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    for &(len, _, packed) in lzsa_samples().iter().filter(|s| (1000..20_000).contains(&s.0)).take(20) {
        // 出力先が 1 バイト足りない
        let mut out = vec![0u8; len - 1];
        assert_eq!(asm_unlzsa2(packed, &mut out), None);
        check(packed, len - 1);
        // 途中で切れている
        for cut in [3, 4, 6, packed.len() / 2, packed.len() - 4] {
            check(&packed[..cut], len);
        }
        // バイトを書き換える
        for _ in 0..30 {
            let mut bad = packed.to_vec();
            let i = rand() % bad.len();
            bad[i] ^= (1 + rand() % 255) as u8;
            check(&bad, len);
        }
    }
    assert_eq!(asm_unlzsa2(b"", &mut [0; 4]), None);
    assert_eq!(asm_unlzsa2(&[0x7b, 0x9e, 0x00, 0, 0, 0], &mut [0; 4]), None); // LZSA1 のヘッダ
    assert_eq!(asm_unlzsa2(&[0x7b, 0x9e, 0x20, 0, 0, 0], &mut [0; 4]), Some(0));
}

fn fs_read_text_files() {
    let hello = fs::read_file("/HELLO.TXT").unwrap();
    assert!(hello.starts_with(b"Hello from the floppy!"));
    let names: Vec<String> = fs::read_dir("/").unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.iter().any(|n| n == "DOCS"), "{names:?}");
    let readme = fs::read_file("/docs/../DOCS/./readme.txt").unwrap();
    assert!(readme.starts_with(b"VibeOS documents."));
}

fn fs_errors() {
    use vibe_fat::FsError;
    assert_eq!(fs::read_file("/NOPE.TXT").err(), Some(fs::VfsError::Fs(FsError::NotFound)));
    assert_eq!(fs::open("/DOCS").err(), Some(fs::VfsError::Fs(FsError::IsADirectory)));
    assert_eq!(fs::read(99, &mut [0u8; 4]).err(), Some(fs::VfsError::BadFd));
    assert!(fs::chdir("/HELLO.TXT").is_err());
}

fn fs_fd_read_in_chunks() {
    let whole = fs::read_file("/KERNEL.SYS").unwrap();
    let fd = fs::open("/KERNEL.SYS").unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 777];
    loop {
        let n = fs::read(fd, &mut buf).unwrap();
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
    }
    fs::close(fd).unwrap();
    assert_eq!(got, whole);
    assert_eq!(fs::close(fd).err(), Some(fs::VfsError::BadFd));
    assert_eq!(fs::open_count(), 0);
}

fn fs_cwd_is_per_task() {
    assert_eq!(fs::cwd(), "/");
    let id = task::spawn("cwd", || {
        fs::chdir("docs").unwrap();
        assert_eq!(fs::cwd(), "/DOCS");
        assert!(fs::read_file("README.TXT").is_ok());
        let fd = fs::open("README.TXT").unwrap();
        assert_eq!(fd, 0);
    });
    task::join(id);
    assert_eq!(fs::cwd(), "/");
    assert_eq!(fs::open_count(), 0, "file table must not be shared");
}

fn run(path: &str, args: &[&str]) -> i32 {
    let id = crate::process::spawn(path, args).unwrap_or_else(|e| panic!("spawn {path}: error {e}"));
    crate::process::wait(id).unwrap()
}

fn process_exit_codes() {
    assert_eq!(run("/BIN/HELLO.ELF", &["hello"]), 0);
    assert_eq!(run("/BIN/FAULT.ELF", &["fault", "exit", "42"]), 42);
    assert_eq!(run("/BIN/FAULT.ELF", &["fault", "null"]), vibe_abi::EXIT_KILLED);
    assert_eq!(run("/BIN/FAULT.ELF", &["fault", "stack"]), vibe_abi::EXIT_KILLED);
}

fn process_spawn_errors() {
    use vibe_abi::err;
    assert_eq!(crate::process::spawn("/BIN/NOPE.ELF", &["x"]).err(), Some(err::ENOENT));
    assert_eq!(crate::process::spawn("/HELLO.TXT", &["x"]).err(), Some(err::ENOEXEC));
    assert_eq!(crate::process::spawn("/KERNEL.SYS", &["x"]).err(), Some(err::ENOEXEC));
    assert_eq!(crate::process::wait(9999).err(), Some(err::ECHILD));
}

/// プロセスが終わると、そのページ・ページテーブル・カーネルスタックがすべて返る
fn process_reclaims_memory() {
    // カーネルヒープが途中で物理ページを取りに行かないよう、先に余裕を持たせておく
    drop(vec![0u8; 512 * 1024]);
    task::yield_now();
    let (before, _) = pmm::stats();
    for _ in 0..5 {
        assert_eq!(run("/BIN/HELLO.ELF", &["hello"]), 0);
        assert_eq!(run("/BIN/FAULT.ELF", &["fault", "kernel"]), vibe_abi::EXIT_KILLED);
    }
    task::yield_now();
    let (after, _) = pmm::stats();
    assert_eq!(before, after, "pages leaked by processes");
}

/// フロッピーから直接読んだ中身が、起動時に RAM ディスクへ読み込まれた中身と同じ
fn fdc_read_matches_ramdisk() {
    use crate::drivers::fdc;
    assert!(fdc::present(), "no floppy drive");
    let info = crate::bootinfo::get();
    // 先頭の 2 トラックと、途中のトラックの一部（トラックをまたぐ読み込み）
    for (lba, count) in [(0u32, 36usize), (700, 30)] {
        let mut buf = vec![0u8; count * 512];
        fdc::read(lba, &mut buf).unwrap();
        // SAFETY: RAM ディスクの範囲の中
        let ram = unsafe { core::slice::from_raw_parts((info.ramdisk_addr + lba * 512) as *const u8, count * 512) };
        assert!(buf == ram, "sectors {lba}+{count} differ");
    }
}

/// 書いたファイルが sync でフロッピーに届く（フロッピーから読み直して確かめる）
fn fs_write_and_sync() {
    use crate::drivers::fdc;
    fs::write_file("/KTEST.TXT", b"written by ktest").unwrap();
    assert_eq!(fs::read_file("/ktest.txt").unwrap(), b"written by ktest");
    assert!(fs::dirty_sectors() > 0);
    let n = fs::sync().unwrap();
    assert!(n > 0);
    assert_eq!(fs::dirty_sectors(), 0);
    // フロッピーのデータ領域を読み直して、書いた文字列があるか
    let mut disk = vec![0u8; 2880 * 512];
    fdc::read(0, &mut disk).unwrap();
    assert!(disk.windows(16).any(|w| w == b"written by ktest"), "data not on the floppy");
    fs::remove("/KTEST.TXT").unwrap();
    fs::sync().unwrap();
}
