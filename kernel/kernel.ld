/* カーネルは 0x100000 に置かれたフラットバイナリとして実行される。
 * ローダはファイルの先頭にジャンプするので、.text.entry を必ず最初に置く。 */
ENTRY(_start)

SECTIONS
{
    . = 0x100000;
    __kernel_start = .;

    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)
    }
    /* ここまで（.text・.rodata）は起動後に変わらない（ktest が KERNEL.SYS の中身と比べる） */
    __rodata_end = .;

    .data : ALIGN(16) {
        *(.data .data.*)
    }

    .bss : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .eh_frame.*)
        *(.comment .note .note.*)
        *(.got .got.plt .igot .igot.plt)
    }
}
