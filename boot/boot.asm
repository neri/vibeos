; VibeOS ブートセクタ（IPL）
;
; BIOS により 0x7C00 にロードされる。FAT12 のルートディレクトリから 2nd ステージ
; LOADER.SYS を探し、クラスタをたどって 0x8000 に読み込んでジャンプする。
;
; BPB は mkfs.fat（Makefile のオプション）と同じ値にする（Makefile で照合している）。
; 実機の BIOS は BPB を検査・書き換えすることがあるので、BPB の領域にはコードも
; データも置かない。ディスクの形と FAT の配置は実行時に BPB から求める。
; 2nd ステージが CPU を確かめるまでは 386 以降の命令を使わない（286 で黙って止まらないように）。

bits 16
org 0x7C00

LOADER_ADDR     equ 0x8000
BUF             equ 0x1000              ; ルートディレクトリ → FAT の読み込み先（〜0x2C00）

    jmp short start
    nop

; --- BPB（オフセット 3〜61） ---------------------------------------------------
oem_name        db "VIBEOS  "
bytes_per_sec   dw 512
sec_per_clus    db 1
reserved_secs   dw 1
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

    ; ルートディレクトリを読む。LBA = 予約セクタ数 + FAT の数 × FAT のセクタ数
    mov al, [num_fats]
    cbw
    mul word [fat_secs]
    add ax, [reserved_secs]
    mov di, [root_entries]
    shr di, 4                       ; 1 セクタに 16 エントリ
    mov bx, BUF
    call read_sectors
    mov [data_lba], ax              ; ルートディレクトリの次からデータ領域

    ; LOADER.SYS を探す
    mov cx, [root_entries]
    mov di, BUF
.find:
    push cx
    push di
    mov si, loader_name
    mov cx, 11
    repe cmpsb
    pop di
    pop cx
    je .found
    add di, 32
    loop .find
    mov si, msg_not_found
    jmp error
.found:
    push word [di + 26]             ; 先頭クラスタ

    ; FAT（1 つ目）を同じバッファに読む
    mov ax, [reserved_secs]
    mov di, [fat_secs]
    mov bx, BUF
    call read_sectors

    ; クラスタをたどって読む。番号が続いているクラスタはまとめて 1 回で読む
    pop ax
    mov bx, LOADER_ADDR
.run:                               ; AX = クラスタ番号
    sub ax, 2
    cmp ax, 0xFF0 - 2               ; 0, 1, 0xFF0〜（予約・不良）なら壊れている
    jae bad_fat
    push ax                         ; この塊の先頭（クラスタ番号 - 2）
    xor cx, cx                      ; CX = この塊のクラスタ数
.extend:
    inc cx
    pop ax
    push ax
    add ax, cx
    inc ax                          ; AX = 塊の最後のクラスタ
    mov dx, ax
    call fat_next
    inc dx
    cmp ax, dx
    je .extend
    pop dx                          ; DX = 塊の先頭 - 2
    push ax                         ; 次のクラスタ（または終わりの印）
    push cx
    mov cl, [sec_per_clus]
    xor ch, ch
    xchg ax, dx
    mul cx
    add ax, [data_lba]              ; AX = 塊の先頭 LBA
    pop di
    xchg ax, di
    mul cx
    xchg ax, di                     ; DI = 塊のセクタ数
    call read_sectors
    pop ax
    cmp ax, 0xFF8                   ; 0xFF8〜 はファイルの終わり
    jb .run

    mov dl, [boot_drive]
    jmp 0x0000:LOADER_ADDR

; AX = クラスタ番号 → AX = FAT12 の次のクラスタ番号（SI を壊す）
; オフセット = n * 3 / 2、奇数なら上位 12bit、偶数なら下位 12bit
fat_next:
    mov si, ax
    shr si, 1
    add si, ax
    test al, 1
    mov ax, [si + BUF]              ; mov はフラグを変えない
    jz .even
    shr ax, 4
.even:
    and ah, 0x0F
    ret

; AX = LBA から DI セクタを ES:BX に読む。終わると AX = 次の LBA, BX = 読んだ分だけ進めた値
; CX, DX, SI, DI, BP を壊す
read_sectors:
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

    mov ax, si                      ; 読み込み先が 64KB をまたぐなら FAT が壊れている
    shl ax, 9                       ; （0 番地からのベクタ表を上書きしないように）
    add ax, bx
    jc bad_fat

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
    jnz read_sectors
    ret

bad_fat:
    mov si, msg_bad_fat
    jmp error
disk_error:
    mov si, msg_disk_error
error:
    lodsb
    test al, al
    jz .halt
    mov ah, 0x0E
    xor bh, bh
    int 0x10
    jmp error
.halt:
    cli
    hlt
    jmp .halt

loader_name     db "LOADER  SYS"
msg_disk_error  db "VibeOS: disk read error", 0
msg_not_found   db "VibeOS: no LOADER.SYS", 0
msg_bad_fat     db "VibeOS: bad FAT", 0
boot_drive      db 0
data_lba        dw 0

    times 510 - ($ - $$) db 0
    dw 0xAA55
