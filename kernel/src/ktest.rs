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
    ("fs_read_text_files", fs_read_text_files),
    ("fs_errors", fs_errors),
    ("fs_fd_read_in_chunks", fs_fd_read_in_chunks),
    ("fs_cwd_is_per_task", fs_cwd_is_per_task),
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
    // カーネル空間で RAM の外にはページテーブルを作らない
    assert!(paging::map(pd, 0x3F00_0000, phys, paging::WRITABLE).is_err());
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
    for _ in 0..5 {
        let ids: Vec<_> = (0..20).map(|_| task::spawn("short", || task::yield_now())).collect();
        for id in ids {
            task::join(id);
        }
    }
    task::yield_now(); // 終わったタスクは次の切り替えで回収される
    let (free_after, _) = pmm::stats();
    assert_eq!(free_before, free_after, "stack pages were not returned");
    assert!(sched::tasks().len() <= 3, "dead tasks remain: {}", sched::tasks().len());
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

fn fs_kernel_file_matches_boot_info() {
    let info = crate::bootinfo::get();
    let st = fs::stat("/KERNEL.SYS").unwrap();
    assert_eq!(st.size, info.kernel_size);
    let data = fs::read_file("/kernel.sys").unwrap();
    // SAFETY: カーネル本体の先頭（ローダがファイルの中身をそのまま置いた）
    let loaded = unsafe { core::slice::from_raw_parts(info.kernel_addr as *const u8, 64) };
    assert_eq!(&data[..64], loaded);
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
