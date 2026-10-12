; テスト用カーネル（qemu-test）にリンクする、ローダと同じ LZSA2 の展開ルーチン（32 ビット版）。
; kernel/build.rs が nasm -f elf32 で組む。ktest がホストで圧縮した境目のデータを展開させる。
;
; isize unlzsa2_call(const u8 *src, usize src_len, u8 *dst, usize dst_cap)
;   展開した大きさ。壊れていれば -1（dst_cap を越えては書かない）

bits 32
section .text

global unlzsa2_call
unlzsa2_call:
    push ebp
    push ebx
    push esi
    push edi
    mov esi, [esp + 20]
    mov ebx, esi
    add ebx, [esp + 24]
    mov edi, [esp + 28]
    mov edx, edi
    add edx, [esp + 32]
    call unlzsa2
    jc .error
    mov eax, edi
    sub eax, [esp + 28]
    jmp .out
.error:
    mov eax, -1
.out:
    pop edi
    pop esi
    pop ebx
    pop ebp
    ret

%include "unlzsa2.inc"

section .bss
unl_sp          resd 1
unl_in_end      resd 1
unl_out_start   resd 1
unl_out_limit   resd 1
unl_nibble      resb 1
