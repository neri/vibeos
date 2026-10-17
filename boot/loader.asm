; VibeOS 2nd ステージローダ
;
; IPL により 0x8000 にロードされる（DL = ブートドライブ）。BIOS が使えるうちに
; 次のことを済ませ、32bit プロテクトモードでカーネルへジャンプする。
;
;   1. シリアル（COM1）初期化。以降の進み具合はシリアルと画面の両方に出す
;      CPU が 486 以降か確かめる（EFLAGS.AC を変えられるか）。カーネルは 486 向けに
;      コンパイルしているので、386 では最初の方の命令で止まってしまい、自分では確かめられない
;   2. A20 ライン有効化（BIOS → キーボードコントローラ → Fast A20）
;   3. Unreal モードに入る（リアルモードのまま 4GB を 32bit アドレスで触れるようにする）
;   4. メモリマップ取得（E820 → E801 → 88h）
;   5. 画面モード 13h に切り替え、進み具合のバーを描く（読み込み中の表示用）
;   6. フロッピー全体を RAMDISK_ADDR に読み込む（RAM ディスク）
;   7. RAM ディスク上の FAT12 から KERNEL.SYS を探して KERNEL_ADDR にコピー。
;      先頭が "VPK1" なら圧縮したカーネル（crates/vibe-pack）: RAM ディスクの後ろの作業用の場所へ
;      写してから LZSA2 で KERNEL_ADDR へ展開し、BCJ を戻して CRC32 を確かめる
;   8. VGA の BIOS の 8x16 のフォントを FONT16_ADDR に写し（int 10h AX=1130h BH=06h）、画面を VGA のモード 12h
;      （640x480・16 色・4 枚の面）にする（docs/plans/vga16.md）
;   9. ブート情報構造体（BI）を埋め、プロテクトモードへ移ってカーネルへジャンプ
;      （EAX = BOOT_MAGIC, EBX = BI のアドレス）

bits 16
org 0x8000

%include "bootinfo.inc"

KERNEL_ADDR     equ 0x00100000
RAMDISK_ADDR    equ 0x00400000
KERNEL_MAX      equ RAMDISK_ADDR - KERNEL_ADDR
BOUNCE_SEG      equ 0x1000              ; ディスク読み込みの中継バッファ（物理 0x10000〜）
BOUNCE_ADDR     equ BOUNCE_SEG * 16
BPB             equ 0x7C00              ; IPL がメモリに残っているので BPB はここから読む
FONT16_ADDR     equ 0x3000              ; VGA の BIOS の 8x16 のフォントの写し（4096 バイト。IPL の作業場所の後ろ）

; 圧縮したカーネルのヘッダ（crates/vibe-pack と同じ形）
VPK_MAGIC       equ 0x314B5056          ; "VPK1"
VPK_FORMAT      equ 4                   ; u8: 2 = LZSA2
VPK_FLAGS       equ 5                   ; u8: bit0 = BCJ
VPK_HEADER_SIZE equ 6                   ; u16
VPK_SIZE        equ 8                   ; u32: 展開後の大きさ
VPK_BCJ_BASE    equ 12                  ; u32
VPK_CRC         equ 16                  ; u32
VPK_MIN_HEADER  equ 20
CRC_TABLE_SIZE  equ 1024
COM1            equ 0x3F8

; 進み具合のバー（モード 13h の座標）
BAR_X           equ 60
BAR_Y           equ 184
BAR_W           equ 200
BAR_H           equ 6

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    cld
    mov [boot_drive], dl

    call serial_init
    mov si, msg_hello
    call print
    call check_cpu

    ; ブート情報構造体をゼロクリア（実機のメモリは初期化されていない）
    mov di, BI
    mov cx, BI_SIZE
    xor al, al
    rep stosb
    movzx eax, byte [boot_drive]
    mov [BI + BI_BOOT_DRIVE], eax

    call enable_a20
    mov si, msg_a20
    call print

    call enter_unreal
    call detect_memory

    call video_init
    call load_disk
    call fs_layout
    call load_kernel

    mov dword [BI + BI_MAGIC], BOOT_MAGIC
    call fetch_font
    call set_video

    mov si, msg_jump
    call print
    jmp enter_protected_mode

; =============================================================================
; 出力（シリアル + BIOS テレタイプ）

serial_init:
    mov dx, COM1 + 1
    xor al, al
    out dx, al                  ; 割り込み無効
    mov dx, COM1 + 3
    mov al, 0x80
    out dx, al                  ; DLAB = 1
    mov dx, COM1 + 0
    mov al, 1
    out dx, al                  ; 115200 bps（分周比 1）
    mov dx, COM1 + 1
    xor al, al
    out dx, al
    mov dx, COM1 + 3
    mov al, 0x03
    out dx, al                  ; 8N1, DLAB = 0
    mov dx, COM1 + 2
    mov al, 0xC7
    out dx, al                  ; FIFO 有効・クリア
    mov dx, COM1 + 4
    mov al, 0x03
    out dx, al                  ; DTR, RTS
    ret

; AL を COM1 に送る。UART が無い・応答しない場合に備えて待ちには上限を付ける
serial_putc:
    push cx
    push dx
    push ax
    mov cx, 0xFFFF
    mov dx, COM1 + 5
.wait:
    in al, dx
    test al, 0x20
    jnz .ready
    loop .wait
.ready:
    pop ax
    mov dx, COM1
    out dx, al
    pop dx
    pop cx
    ret

putc:
    pusha
    call serial_putc
    cmp byte [text_off], 0              ; モード 12h にした後は BIOS の文字表示を使わない
    jne .done
    mov ah, 0x0E
    xor bh, bh
    mov bl, 15                  ; グラフィックモードでの文字色
    int 0x10
.done:
    popa
    ret

; DS:SI の 0 終端文字列を出力
print:
    push ax
    push si
.loop:
    lodsb
    test al, al
    jz .done
    call putc
    jmp .loop
.done:
    pop si
    pop ax
    ret

; EAX を 16 進 8 桁で出力
print_hex32:
    pushad
    mov cx, 8
.loop:
    rol eax, 4
    push eax
    and al, 0x0F
    add al, '0'
    cmp al, '9'
    jbe .digit
    add al, 'A' - '9' - 1
.digit:
    call putc
    pop eax
    loop .loop
    popad
    ret

; SI = メッセージ。エラーを表示して停止する
fatal:
    push si
    mov si, msg_error
    call print
    pop si
    call print
.halt:
    cli
    hlt
    jmp .halt

; =============================================================================
; CPU の確認: 486 以降か（386 では EFLAGS の AC ビット（bit18）を変えられない）。
; CPUID の無い初期の 486 もあるので CPUID は求めない

check_cpu:
    pushfd
    pop eax
    mov ecx, eax
    xor eax, 1 << 18
    push eax
    popfd
    pushfd
    pop eax
    push ecx
    popfd
    xor eax, ecx
    test eax, 1 << 18
    jz .too_old
    ret
.too_old:
    mov si, msg_old_cpu
    jmp fatal

; =============================================================================
; A20

enable_a20:
    call check_a20
    jnz .done

    mov ax, 0x2401              ; BIOS
    int 0x15
    call check_a20
    jnz .done

    call a20_kbc                ; キーボードコントローラ
    call check_a20
    jnz .done

    in al, 0x92                 ; Fast A20
    test al, 2
    jnz .fast_skip
    or al, 2
    and al, 0xFE                ; bit0 はシステムリセットなので必ず 0
    out 0x92, al
.fast_skip:
    mov cx, 0x100
.fast_wait:
    call check_a20
    jnz .done
    loop .fast_wait

    mov si, msg_a20_fail
    jmp fatal
.done:
    ret

; A20 が有効なら ZF = 0
; 0000:0500 と FFFF:0510 は A20 が無効だと同じ物理アドレスになる
check_a20:
    pushf
    push ds
    push es
    push di
    push si
    push ax
    cli
    xor ax, ax
    mov es, ax
    mov di, 0x0500
    not ax
    mov ds, ax
    mov si, 0x0510
    mov al, [es:di]
    push ax
    mov al, [ds:si]
    push ax
    mov byte [es:di], 0x00
    mov byte [ds:si], 0xFF
    cmp byte [es:di], 0xFF      ; 書き換わっていたら折り返している = 無効
    pop ax
    mov [ds:si], al
    pop ax
    mov [es:di], al
    pop ax
    pop si
    pop di
    pop es
    pop ds
    je .disabled
    popf
    test sp, sp                 ; ZF = 0（SP は 0 にならない）
    ret
.disabled:
    popf
    cmp al, al                  ; ZF = 1
    ret

a20_kbc:
    cli
    call kbc_wait_input
    mov al, 0xAD                ; キーボード無効
    out 0x64, al
    call kbc_wait_input
    mov al, 0xD0                ; 出力ポート読み出し
    out 0x64, al
    call kbc_wait_output
    in al, 0x60
    push ax
    call kbc_wait_input
    mov al, 0xD1                ; 出力ポート書き込み
    out 0x64, al
    call kbc_wait_input
    pop ax
    or al, 2                    ; A20
    out 0x60, al
    call kbc_wait_input
    mov al, 0xAE                ; キーボード有効
    out 0x64, al
    call kbc_wait_input
    sti
    ret

kbc_wait_input:                 ; 入力バッファが空くまで待つ（上限あり）
    push cx
    mov cx, 0xFFFF
.loop:
    in al, 0x64
    test al, 2
    jz .done
    loop .loop
.done:
    pop cx
    ret

kbc_wait_output:                ; 出力バッファにデータが来るまで待つ（上限あり）
    push cx
    mov cx, 0xFFFF
.loop:
    in al, 0x64
    test al, 1
    jnz .done
    loop .loop
.done:
    pop cx
    ret

; =============================================================================
; Unreal モード
;
; 一度プロテクトモードに入ってデータセグメントに 4GB リミットを読み込み、すぐ
; リアルモードへ戻る。セグメントのリミットは保持されるので、リアルモードのまま
; DS/ES 経由で 32bit アドレス（a32 プレフィックス）を使える。
; BIOS 呼び出しがリミットを戻してしまうことがあるので、使う直前に毎回呼ぶ。

enter_unreal:
    pushad
    push ds
    push es
    cli
    lgdt [gdt_desc]
    mov eax, cr0
    or al, 1
    mov cr0, eax
    jmp GDT_CODE16:.pm16
.pm16:
    mov bx, GDT_DATA32
    mov ds, bx
    mov es, bx
    and al, 0xFE
    mov cr0, eax
    jmp 0x0000:.rm
.rm:
    pop es
    pop ds
    sti
    popad
    ret

; =============================================================================
; メモリマップ

detect_memory:
    mov di, BI + BI_MMAP
    xor ebx, ebx
    xor bp, bp                  ; BP = エントリ数
.e820:
    mov eax, 0xE820
    mov ecx, E820_ENTRY_SIZE
    mov edx, 0x534D4150         ; 'SMAP'
    mov dword [di + 20], 1      ; ACPI 3.0 拡張属性の既定値（有効）
    int 0x15
    jc .e820_end
    cmp eax, 0x534D4150
    jne .e820_end
    mov eax, [di + 8]
    or eax, [di + 12]
    jz .e820_skip               ; 長さ 0 のエントリは捨てる
    cmp ecx, 20
    ja .e820_keep
    mov dword [di + 20], 1      ; 20 バイトしか返さない BIOS
.e820_keep:
    inc bp
    add di, E820_ENTRY_SIZE
    cmp bp, MMAP_MAX
    jae .e820_end
.e820_skip:
    test ebx, ebx
    jnz .e820
.e820_end:
    test bp, bp
    jnz .done

    ; E820 が使えない古い BIOS: E801 → 88h で近似のマップを作る
    int 0x12                    ; AX = 640KB 以下の KB 数
    movzx edx, ax
    shl edx, 10
    xor eax, eax
    call add_mmap_entry         ; [0, 低位メモリ)

    xor cx, cx
    xor dx, dx
    mov ax, 0xE801
    int 0x15
    jc .try88
    test cx, cx
    jnz .e801_cxdx
    test dx, dx
    jnz .e801_cxdx
    mov cx, ax
    mov dx, bx
.e801_cxdx:
    push dx
    movzx edx, cx               ; 1MB〜16MB（KB 単位）
    shl edx, 10
    mov eax, 0x100000
    call add_mmap_entry
    pop dx
    movzx edx, dx               ; 16MB〜（64KB 単位）
    test edx, edx
    jz .done
    shl edx, 16
    mov eax, 0x1000000
    call add_mmap_entry
    jmp .done

.try88:
    mov ah, 0x88
    int 0x15
    jc .fail
    movzx edx, ax
    shl edx, 10
    mov eax, 0x100000
    call add_mmap_entry
    jmp .done

.fail:
    mov si, msg_mem_fail
    jmp fatal

.done:
    movzx eax, bp
    mov [BI + BI_MMAP_COUNT], eax
    mov si, msg_mem
    call print
    call print_hex32
    mov si, msg_crlf
    call print
    ret

; EAX = 先頭, EDX = 長さ の使用可能領域を DI に追加（BP を数える）
add_mmap_entry:
    test edx, edx
    jz .skip
    mov [di], eax
    mov dword [di + 4], 0
    mov [di + 8], edx
    mov dword [di + 12], 0
    mov dword [di + 16], 1      ; 使用可能
    mov dword [di + 20], 1
    add di, E820_ENTRY_SIZE
    inc bp
.skip:
    ret

; [KERNEL_ADDR, EAX) が 1 つの使用可能領域に収まるか
check_memory_fits:
    mov [need_end], eax
    mov si, BI + BI_MMAP
    mov cx, [BI + BI_MMAP_COUNT]
.loop:
    jcxz .fail
    cmp dword [si + 16], 1      ; 使用可能か
    jne .next
    cmp dword [si + 4], 0       ; 4GB 以上から始まる領域は対象外
    jne .next
    mov eax, [si]
    cmp eax, KERNEL_ADDR
    ja .next
    cmp dword [si + 12], 0      ; 長さが 4GB 以上なら十分
    jne .ok
    add eax, [si + 8]
    jc .ok
    cmp eax, [need_end]
    jae .ok
.next:
    add si, E820_ENTRY_SIZE
    dec cx
    jmp .loop
.fail:
    mov si, msg_no_memory
    jmp fatal
.ok:
    ret

; =============================================================================
; 画面

video_init:
    mov ax, 0x0013
    int 0x10
    ; バーの枠
    mov ax, BAR_X - 2
    mov bx, BAR_Y - 2
    mov cx, BAR_W + 4
    mov dx, BAR_H + 4
    mov bp, 8                   ; 暗い灰色
    call fill_rect
    mov ax, BAR_X - 1
    mov bx, BAR_Y - 1
    mov cx, BAR_W + 2
    mov dx, BAR_H + 2
    mov bp, 0                   ; 黒
    call fill_rect
    ret

; AX = x, BX = y, CX = 幅, DX = 高さ, BP の下位 8bit = 色
fill_rect:
    pusha
    push es
    jcxz .done
    test dx, dx
    jz .done
    push ax
    mov ax, 0xA000
    mov es, ax
    mov ax, 320
    mul bx                      ; DX:AX = y * 320（DX は壊れる）
    pop di
    add di, ax                  ; DI = y * 320 + x
    mov bx, sp
    mov dx, [ss:bx + 2 + 10]    ; pusha で積んだ元の DX（高さ）を取り直す
    mov ax, bp
.row:
    push cx
    push di
    rep stosb
    pop di
    pop cx
    add di, 320
    dec dx
    jnz .row
.done:
    pop es
    popa
    ret

; 読み込んだセクタ数に応じてバーを伸ばす
draw_progress:
    pushad
    mov eax, [cur_lba]
    mov ecx, BAR_W
    mul ecx
    div dword [disk_sectors]
    mov cx, ax                  ; 幅
    mov ax, BAR_X
    mov bx, BAR_Y
    mov dx, BAR_H
    mov bp, 9                   ; 明るい青
    call fill_rect
    popad
    ret

; =============================================================================
; フロッピー全体を RAM ディスクへ

load_disk:
    movzx eax, word [BPB + 0x13]        ; 総セクタ数（16bit）
    test eax, eax
    jnz .have_total
    mov eax, [BPB + 0x20]               ; 総セクタ数（32bit）
.have_total:
    mov [disk_sectors], eax
    shl eax, 9
    mov [BI + BI_RAMDISK_SIZE], eax
    mov dword [BI + BI_RAMDISK_ADDR], RAMDISK_ADDR
    add eax, RAMDISK_ADDR
    call check_memory_fits

    mov dword [cur_lba], 0
    mov dword [dest], RAMDISK_ADDR
.loop:
    mov eax, [cur_lba]
    cmp eax, [disk_sectors]
    jae .done
    call read_chunk                     ; ECX = 読んだセクタ数
    call enter_unreal
    push ecx
    mov esi, BOUNCE_ADDR
    mov edi, [dest]
    shl ecx, 7                          ; セクタ数 → dword 数
    a32 rep movsd
    mov [dest], edi
    pop ecx
    add [cur_lba], ecx
    call draw_progress
    jmp .loop
.done:
    mov si, msg_disk
    call print
    mov eax, [BI + BI_RAMDISK_SIZE]
    call print_hex32
    mov si, msg_crlf
    call print
    ret

; EAX = LBA から、トラックの終わり（またはディスクの終わり）までを中継バッファに読む
; 出力: ECX = 読んだセクタ数
read_chunk:
    push eax
    xor edx, edx
    movzx ebx, word [BPB + 0x18]        ; トラックあたりセクタ数
    div ebx                             ; EAX = LBA / SPT, EDX = LBA % SPT
    mov ecx, ebx
    sub ecx, edx                        ; このトラックの残り
    mov esi, [disk_sectors]
    sub esi, [cur_lba]                  ; ディスクの残り
    cmp ecx, esi
    jbe .count_ok
    mov ecx, esi
.count_ok:
    mov [chunk_count], ecx
    mov [chunk_sector], dl
    inc byte [chunk_sector]             ; 1 始まり
    xor edx, edx
    movzx ebx, word [BPB + 0x1A]        ; ヘッド数
    div ebx                             ; EAX = シリンダ, EDX = ヘッド
    mov [chunk_cyl], ax
    mov [chunk_head], dl

    mov byte [retries], 3
.retry:
    push es
    mov ax, BOUNCE_SEG
    mov es, ax
    xor bx, bx
    mov al, [chunk_count]
    mov ah, 0x02
    mov cx, [chunk_cyl]
    xchg ch, cl                         ; CH = シリンダ下位 8bit
    shl cl, 6                           ; CL bit6-7 = シリンダ上位 2bit
    or cl, [chunk_sector]
    mov dh, [chunk_head]
    mov dl, [boot_drive]
    int 0x13
    pop es
    jnc .ok
    dec byte [retries]
    jz .error
    xor ah, ah
    mov dl, [boot_drive]
    int 0x13                            ; リセットしてやり直す
    jmp .retry
.ok:
    mov ecx, [chunk_count]
    pop eax
    ret
.error:
    mov si, msg_disk_fail
    jmp fatal

; =============================================================================
; RAM ディスク上の FAT12 から KERNEL.SYS を KERNEL_ADDR へコピー

; RAM ディスク上の FAT12 の配置を BPB から求める
fs_layout:
    movzx eax, word [BPB + 0x0E]        ; 予約セクタ数 = FAT の先頭 LBA
    mov [fat_lba], eax
    movzx ebx, byte [BPB + 0x10]        ; FAT の数
    movzx ecx, word [BPB + 0x16]        ; FAT 1 つのセクタ数
    imul ebx, ecx
    add eax, ebx
    mov [root_lba], eax
    movzx ecx, word [BPB + 0x11]        ; ルートディレクトリのエントリ数
    add ecx, 15
    shr ecx, 4                          ; 1 セクタに 16 エントリ
    add eax, ecx
    mov [data_lba], eax
    ret

; ルートディレクトリから EDI（11 バイトの名前）を探す。見つかれば CF = 0 で ESI = エントリ（RAM ディスク上）
find_root_file:
    call enter_unreal
    mov esi, [root_lba]
    shl esi, 9
    add esi, RAMDISK_ADDR
    movzx ecx, word [BPB + 0x11]
.search:
    cmp byte [esi], 0                   ; 以降は未使用
    je .not_found
    test byte [esi + 11], 0x18          ; ディレクトリ・ボリュームラベルは除く
    jnz .search_next
    push ecx
    push esi
    push edi
    mov ecx, 11
    a32 repe cmpsb
    pop edi
    pop esi
    pop ecx
    je .found
.search_next:
    add esi, 32
    dec ecx
    jnz .search
.not_found:
    stc
    ret
.found:
    clc
    ret

load_kernel:
    mov edi, kernel_name
    call find_root_file
    jnc .found
    mov si, msg_no_kernel
    jmp fatal

.found:
    mov eax, [esi + 28]                 ; ファイルの大きさ
    test eax, eax
    jz .bad_kernel
    cmp eax, KERNEL_MAX
    ja .too_big
    mov [file_size], eax
    movzx eax, word [esi + 26]          ; 先頭クラスタ
    mov [first_cluster], eax
    mov dword [BI + BI_KERNEL_ADDR], KERNEL_ADDR

    ; 先頭のクラスタが "VPK1" で始まれば圧縮したカーネル
    cmp eax, 2
    jb .bad_kernel
    call cluster_addr
    cmp dword [file_size], VPK_MIN_HEADER
    jb .plain
    cmp dword [esi], VPK_MAGIC
    je load_packed_kernel

.plain:
    mov eax, [file_size]
    mov [BI + BI_KERNEL_SIZE], eax
    mov eax, [first_cluster]
    mov edi, KERNEL_ADDR
    mov ecx, KERNEL_MAX
    call copy_chain
    jmp kernel_loaded
.too_big:
    mov si, msg_kernel_big
    jmp fatal
.bad_kernel:
    jmp bad_kernel

; 圧縮したカーネル: RAM ディスクの後ろ（64KB 境界）の作業用の場所へ写し、KERNEL_ADDR へ展開する。
; 作業用の場所 = [圧縮したファイル（クラスタ単位）][CRC32 の表 1KB]
load_packed_kernel:
    mov si, msg_kernel_packed
    call print
    mov eax, [file_size]
    call print_hex32
    mov si, msg_crlf
    call print

    mov eax, RAMDISK_ADDR + 0xFFFF
    add eax, [BI + BI_RAMDISK_SIZE]
    and eax, 0xFFFF0000
    mov [work_addr], eax
    call cluster_bytes                  ; ECX = クラスタの大きさ
    mov eax, [file_size]
    add eax, ecx
    dec eax
    xor edx, edx
    div ecx
    imul eax, ecx                       ; ファイルの大きさをクラスタ単位に切り上げ
    mov [work_size], eax
    add eax, [work_addr]
    mov [crc_table], eax
    add eax, CRC_TABLE_SIZE
    call check_memory_fits

    mov eax, [first_cluster]
    mov edi, [work_addr]
    mov ecx, [work_size]
    call copy_chain

    ; ヘッダを確かめる
    mov esi, [work_addr]
    cmp byte [esi + VPK_FORMAT], 2
    jne bad_kernel
    test byte [esi + VPK_FLAGS], 0xFE
    jnz bad_kernel
    movzx eax, word [esi + VPK_HEADER_SIZE]
    cmp eax, VPK_MIN_HEADER
    jb bad_kernel
    cmp eax, [file_size]
    ja bad_kernel
    mov ecx, [esi + VPK_SIZE]
    test ecx, ecx
    jz bad_kernel
    cmp ecx, KERNEL_MAX
    ja load_kernel.too_big
    mov [BI + BI_KERNEL_SIZE], ecx

    ; LZSA2 で展開（出力の上限 = 展開後の大きさ。ちょうど埋まらなければ壊れている）
    add esi, eax
    mov ebx, [work_addr]
    add ebx, [file_size]
    mov edi, KERNEL_ADDR
    lea edx, [edi + ecx]
    call unlzsa2
    jc bad_kernel
    mov eax, KERNEL_ADDR
    add eax, [BI + BI_KERNEL_SIZE]
    cmp edi, eax
    jne bad_kernel

    ; BCJ を戻す
    mov esi, [work_addr]
    test byte [esi + VPK_FLAGS], 1
    jz .crc
    mov ebx, [esi + VPK_BCJ_BASE]
    mov edi, KERNEL_ADDR
    mov ecx, [BI + BI_KERNEL_SIZE]
    call unbcj

.crc:
    mov edi, [crc_table]
    call crc_make_table
    mov ebx, [crc_table]
    mov esi, KERNEL_ADDR
    mov ecx, [BI + BI_KERNEL_SIZE]
    call crc32
    mov esi, [work_addr]
    cmp eax, [esi + VPK_CRC]
    jne bad_kernel
    jmp kernel_loaded

kernel_loaded:
    mov si, msg_kernel
    call print
    mov eax, [BI + BI_KERNEL_SIZE]
    call print_hex32
    mov si, msg_crlf
    call print
    ret

bad_kernel:
    mov si, msg_bad_kernel
    jmp fatal

; ECX = 1 クラスタのバイト数
cluster_bytes:
    movzx ecx, byte [BPB + 0x0D]
    shl ecx, 9
    ret

; EAX = クラスタ番号 → ESI = RAM ディスク上の番地
cluster_addr:
    push eax
    push ecx
    sub eax, 2
    movzx ecx, byte [BPB + 0x0D]        ; クラスタあたりセクタ数
    imul eax, ecx
    add eax, [data_lba]
    shl eax, 9
    lea esi, [eax + RAMDISK_ADDR]
    pop ecx
    pop eax
    ret

; EAX = 先頭クラスタから FAT をたどって EDI へ写す。ECX = 写してよいバイト数の上限
; （FAT が壊れていて無限ループ・書きすぎにならないため。越えたら壊れている）
copy_chain:
    call enter_unreal
    mov [copy_left], ecx
.chain:
    cmp eax, 2
    jb bad_kernel
    cmp eax, 0xFF8                      ; 終端
    jae .done
    call cluster_bytes
    sub [copy_left], ecx
    jb bad_kernel

    call cluster_addr
    shr ecx, 2                          ; dword 数
    a32 rep movsd

    ; FAT12 の次のエントリ: オフセット = n * 3 / 2、奇数なら上位 12bit、偶数なら下位 12bit
    mov ebx, eax
    shr ebx, 1
    add ebx, eax
    mov esi, [fat_lba]
    shl esi, 9
    add esi, RAMDISK_ADDR
    mov dx, [esi + ebx]
    test al, 1
    jz .even
    shr dx, 4
    jmp .next_cluster
.even:
    and dx, 0x0FFF
.next_cluster:
    movzx eax, dx
    jmp .chain
.done:
    ret

; BCJ を戻す（crates/vibe-pack の bcj_decode と同じ決まり）。
; EDI = データ, ECX = 大きさ, EBX = データの先頭が置かれる番地
unbcj:
    cmp ecx, 5
    jb .done
    lea ebp, [edi + ecx - 4]            ; i + 5 <= 大きさ の間
    mov esi, edi
    mov edx, edi                        ; ここより前からは書き換えない（直さなかった E8/E9 + 4）
    sub ebx, edi                        ; ESI + EBX = その位置が置かれる番地
.loop:
    cmp esi, ebp
    jae .done
    mov al, [esi]
    and al, 0xFE
    cmp al, 0xE8                        ; E8 (call) / E9 (jmp)
    jne .next
    cmp esi, edx
    jb .keep
    mov al, [esi + 4]
    inc al
    cmp al, 1                           ; 上位 1 バイトが 00 か FF
    ja .keep
    mov eax, [esi + 1]
    lea ecx, [esi + ebx + 5]
    sub eax, ecx                        ; 絶対番地 → 相対番地
    shl eax, 7                          ; 25 ビットで符号を広げる
    sar eax, 7
    mov [esi + 1], eax
    add esi, 5
    jmp .loop
.keep:
    lea edx, [esi + 4]
.next:
    inc esi
    jmp .loop
.done:
    ret

; CRC32 の表（256 項目）を EDI に作る
crc_make_table:
    xor ecx, ecx
.entry:
    mov eax, ecx
    mov dl, 8
.bit:
    shr eax, 1
    jnc .no_xor
    xor eax, 0xEDB88320
.no_xor:
    dec dl
    jnz .bit
    mov [edi + ecx * 4], eax
    inc ecx
    cmp ecx, 256
    jb .entry
    ret

; ESI = データ, ECX = 大きさ, EBX = 表 → EAX = CRC32（zlib と同じ）
crc32:
    mov eax, 0xFFFFFFFF
    test ecx, ecx
    jz .done
.loop:
    movzx edx, byte [esi]
    xor dl, al
    shr eax, 8
    xor eax, [ebx + edx * 4]
    inc esi
    dec ecx
    jnz .loop
.done:
    not eax
    ret

%include "unlzsa2.inc"

; =============================================================================
; VGA の BIOS の 8x16 のフォント（256 字 × 16 バイト。bit 7 が左端）を FONT16_ADDR に写し、ブート情報に
; 場所を書く。BIOS が答えなければ 0 のまま（カーネルは 8x8 を縦に 2 倍して使う）

fetch_font:
    push es
    push bp
    ; 答えの CX は今の画面の字の高さで、頼んだフォントのものではない。ES:BP が変わったかで確かめる
    ; （中身はカーネルが 'A' に絵があるかで確かめる）
    xor ax, ax
    mov es, ax
    xor bp, bp
    mov ax, 0x1130
    mov bh, 0x06                        ; 8x16 のフォント
    int 0x10                            ; ES:BP = フォント
    mov ax, es
    or ax, bp
    jz .fail
    push ds
    push es
    pop ds
    mov si, bp
    xor ax, ax
    mov es, ax
    mov di, FONT16_ADDR
    mov cx, 256 * 16 / 2
    rep movsw
    pop ds
    mov dword [BI + BI_FONT16], FONT16_ADDR
    mov si, msg_font
    call print
    jmp .done
.fail:
    mov si, msg_no_font
    call print
.done:
    pop bp
    pop es
    ret

; =============================================================================
; 画面: VGA のモード 12h（640x480・16 色・4 枚の面）。どの VGA にもあるモードなので選ばない
; （VESA の画面と 320x200 は無くした。docs/plans/vga16.md、DECISIONS.md D-071）

set_video:
    mov si, msg_mode_12h
    call print
    mov ax, 0x0012
    int 0x10
    mov byte [text_off], 1              ; 以降は BIOS の文字表示で画面を汚さない
    mov dword [BI + BI_VRAM_ADDR], 0xA0000
    mov word [BI + BI_SCREEN_W], 640
    mov word [BI + BI_SCREEN_H], 480
    mov dword [BI + BI_PITCH], 80       ; 1 枚の面の 1 行のバイト数
    mov dword [BI + BI_BPP], 4
    ret

; =============================================================================
; プロテクトモードへ

enter_protected_mode:
    cli
    in al, 0x70                 ; NMI 禁止（カーネルが IDT を用意してから戻す）
    or al, 0x80
    out 0x70, al
    in al, 0x71
    lgdt [gdt_desc]
    mov eax, cr0
    or al, 1
    mov cr0, eax
    jmp GDT_CODE32:pm32

bits 32
pm32:
    mov ax, GDT_DATA32
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x7C00
    mov eax, BOOT_MAGIC
    mov ebx, BI
    mov ecx, KERNEL_ADDR
    jmp ecx
bits 16

; =============================================================================
; データ

align 8
gdt:
    dq 0
    dq 0x00CF9A000000FFFF       ; 0x08: 32bit コード, base 0, limit 4GB
    dq 0x00CF92000000FFFF       ; 0x10: 32bit データ, base 0, limit 4GB
    dq 0x00009A000000FFFF       ; 0x18: 16bit コード, base 0, limit 64KB
gdt_end:
gdt_desc:
    dw gdt_end - gdt - 1
    dd gdt

GDT_CODE32      equ 0x08
GDT_DATA32      equ 0x10
GDT_CODE16      equ 0x18

kernel_name     db "KERNEL  SYS"

msg_hello       db "VIBEOS LOADER", 13, 10, 0
msg_a20         db "A20 enabled", 13, 10, 0
msg_mem         db "memory map entries: ", 0
msg_disk        db "disk loaded, bytes: ", 0
msg_kernel      db "kernel loaded, bytes: ", 0
msg_kernel_packed db "kernel compressed, bytes: ", 0
msg_jump        db "jumping to kernel", 13, 10, 0
msg_mode_12h    db "video: 640x480x4", 13, 10, 0
msg_font        db "font: VGA BIOS 8x16", 13, 10, 0
msg_no_font     db "font: no 8x16 font in the VGA BIOS", 13, 10, 0
msg_crlf        db 13, 10, 0
msg_error       db 13, 10, "LOADER ERROR: ", 0
msg_a20_fail    db "cannot enable A20", 13, 10, 0
msg_old_cpu     db "VibeOS needs a 486 or newer CPU", 13, 10, 0
msg_mem_fail    db "cannot get memory map", 13, 10, 0
msg_no_memory   db "not enough memory", 13, 10, 0
msg_disk_fail   db "disk read error", 13, 10, 0
msg_no_kernel   db "KERNEL.SYS not found", 13, 10, 0
msg_kernel_big  db "KERNEL.SYS too big", 13, 10, 0
msg_bad_kernel  db "KERNEL.SYS is broken", 13, 10, 0

boot_drive      db 0
retries         db 0
text_off        db 0                    ; 1 = BIOS の文字表示を使わない（画面をモード 12h にした後）
chunk_sector    db 0
chunk_head      db 0
chunk_cyl       dw 0
align 4
chunk_count     dd 0
disk_sectors    dd 0
cur_lba         dd 0
dest            dd 0
need_end        dd 0
fat_lba         dd 0
root_lba        dd 0
data_lba        dd 0
copy_left       dd 0
file_size       dd 0
first_cluster   dd 0
work_addr       dd 0
work_size       dd 0
crc_table       dd 0
; unlzsa2.inc の作業用
unl_sp          dd 0
unl_in_end      dd 0
unl_out_start   dd 0
unl_out_limit   dd 0
unl_nibble      db 0

