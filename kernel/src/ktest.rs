//! カーネル内テスト（qemu-test 機能のときだけ）
//!
//! 起動処理が終わった後に全テストを順に実行し、すべて通れば QEMU を終了コード 33 で終える。
//! 失敗はテストの中で panic させる（panic 処理が終了コード 35 で QEMU を終える）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

use crate::drivers::qemu;
use crate::mm::{heap, paging, pmm};

const TESTS: &[(&str, fn())] = &[
    ("heap_vec", heap_vec),
    ("heap_string", heap_string),
    ("heap_btreemap", heap_btreemap),
    ("heap_grows", heap_grows),
    ("heap_reuses_freed_memory", heap_reuses_freed_memory),
    ("pmm_alloc_free", pmm_alloc_free),
    ("paging_map_unmap", paging_map_unmap),
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
