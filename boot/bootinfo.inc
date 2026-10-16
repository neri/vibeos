; ブート情報構造体（ローダ → カーネル）
; kernel/src/bootinfo.rs の BootInfo と同じレイアウトにすること。

BI              equ 0x0500              ; 置き場所（物理アドレス）
BOOT_MAGIC      equ 0x45424956          ; "VIBE"（リトルエンディアン）
MMAP_MAX        equ 32
E820_ENTRY_SIZE equ 24                  ; base u64, len u64, type u32, acpi u32

BI_MAGIC        equ 0x000               ; u32
BI_BOOT_DRIVE   equ 0x004               ; u32
BI_MMAP_COUNT   equ 0x008               ; u32
BI_MMAP         equ 0x00C               ; [E820Entry; 32]
BI_RAMDISK_ADDR equ 0x30C               ; u32
BI_RAMDISK_SIZE equ 0x310               ; u32
BI_VRAM_ADDR    equ 0x314               ; u32
BI_SCREEN_W     equ 0x318               ; u16
BI_SCREEN_H     equ 0x31A               ; u16
BI_KERNEL_ADDR  equ 0x31C               ; u32
BI_KERNEL_SIZE  equ 0x320               ; u32
BI_PITCH        equ 0x324               ; u32（画面 1 行のバイト数）
BI_BPP          equ 0x328               ; u32（1 ピクセルのビット数: 8・32、モード 12h は 4）
BI_FONT16       equ 0x32C               ; u32（VGA の BIOS の 8x16 のフォント 4096 バイトの場所。無ければ 0）
BI_SIZE         equ 0x330
