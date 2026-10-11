; VibeOS ブートセクタ（IPL）
;
; BIOS により 0x7C00 にロードされる。予約領域（LBA 1〜63）にある 2nd ステージを
; 0x8000 に読み込んでジャンプするだけ。FAT は解析しない。
;
; BPB は mkfs.fat（Makefile のオプション）と同じ値にする。実機の BIOS は BPB を
; 検査・書き換えすることがあるので、BPB の領域にはコードもデータも置かない。
; ディスクの形（トラックあたりセクタ数・ヘッド数）は実行時に BPB から読む。

bits 16
org 0x7C00

LOADER_ADDR     equ 0x8000
LOADER_SECTORS  equ 63

    jmp short start
    nop

; --- BPB（オフセット 3〜61） ---------------------------------------------------
oem_name        db "VIBEOS  "
bytes_per_sec   dw 512
sec_per_clus    db 1
reserved_secs   dw 64
num_fats        db 2
root_entries    dw 224
total_secs16    dw 2880
media           db 0xF0
fat_secs        dw 9
secs_per_track  dw 18
num_heads       dw 2
hidden_secs     dd 0
total_secs32    dd 0
drive_num       db 0
reserved1       db 0
boot_sig        db 0x29
volume_id       dd 0x56494245
volume_label    db "VIBEOS     "
fs_type         db "FAT12   "

; --- コード ------------------------------------------------------------------
start:
    cli
    jmp 0x0000:.normalized          ; CS:IP を 0000:7C00 系に揃える（07C0:0000 で来る BIOS もある）
.normalized:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    sti
    cld
    mov [boot_drive], dl            ; BIOS が渡したブートドライブ番号

    mov ax, 1                       ; AX = 次に読む LBA
    mov bx, LOADER_ADDR             ; ES:BX = 読み込み先
    mov di, LOADER_SECTORS          ; DI = 残りセクタ数
.next:
    push ax
    ; LBA → CHS。1 回の読み込みはトラックの終わりまで（トラックをまたぐ読み込みは BIOS によっては失敗する）
    xor dx, dx
    div word [secs_per_track]       ; AX = LBA / SPT, DX = LBA % SPT
    mov si, [secs_per_track]
    sub si, dx                      ; SI = このトラックの残りセクタ数
    cmp si, di
    jbe .count_ok
    mov si, di
.count_ok:
    mov cl, dl
    inc cl                          ; CL = セクタ番号（1 始まり）
    xor dx, dx
    div word [num_heads]            ; AX = シリンダ, DX = ヘッド
    mov ch, al
    mov dh, dl
    mov dl, [boot_drive]

    mov bp, 3                       ; リトライ回数
.retry:
    mov ax, si
    mov ah, 0x02
    pusha
    int 0x13
    popa                            ; popa はフラグを変えないので CF はそのまま
    jnc .read_ok
    dec bp
    jz disk_error
    pusha
    xor ah, ah                      ; ドライブをリセットしてやり直す
    int 0x13
    popa
    jmp .retry
.read_ok:
    pop ax
    add ax, si
    sub di, si
    mov dx, si
    shl dx, 9
    add bx, dx
    test di, di
    jnz .next

    mov dl, [boot_drive]
    jmp 0x0000:LOADER_ADDR

disk_error:
    mov si, msg_disk_error
.print:
    lodsb
    test al, al
    jz .halt
    mov ah, 0x0E
    xor bh, bh
    int 0x10
    jmp .print
.halt:
    cli
    hlt
    jmp .halt

msg_disk_error  db "VibeOS: disk read error", 0
boot_drive      db 0

    times 510 - ($ - $$) db 0
    dw 0xAA55
