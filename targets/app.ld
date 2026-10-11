/* ユーザアプリ: 0x40000000 から。コード（読み込み専用）とデータ（書き込み可）を別のページにする */
ENTRY(_start)

SECTIONS
{
    . = 0x40000000;

    .text : {
        *(.text.entry)
        *(.text .text.*)
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)
    }

    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
    }

    .bss : ALIGN(16) {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .eh_frame.*)
        *(.comment .note .note.*)
    }
}
