; LZSA2 のストリーム形式の展開（docs/plans/compress.md, crates/vibe-lzsa と同じ形式）
;
; 32 ビットのレジスタと番地だけを使い、`bits 16`（unreal モードのローダ。NASM が 66/67 の
; 接頭辞を付ける）でも `bits 32`（テスト用カーネルの ktest）でも同じ動きになるように書く。
; DS・ES は 0 から始まる 4GB のセグメントであること。DF = 0 にする。
;
; 入力: ESI = 圧縮データ（3 バイトのヘッダから）, EBX = 圧縮データの終わり
;       EDI = 出力先, EDX = 出力の上限（ここより先には書かない）
; 出力: CF = 0 なら成功で EDI = 書き終えた所。CF = 1 なら壊れている（出力の上限は越えていない）
; 壊す: EAX, EBX, ECX, EDX, ESI, EBP
;
; 作業用の変数は取り込む側で用意する（コードの中に置くと、カーネルでは書き込めないため）:
;   unl_sp（dd）, unl_in_end（dd）, unl_out_start（dd）, unl_out_limit（dd）, unl_nibble（db）
;
; 1 つのブロックの中のレジスタ: ESI = 入力, EBP = ブロックの終わり, EDI = 出力,
; EDX = 前回のオフセット（rep-match。ブロックごとに 0 から）, BL = トークン

; 1 バイト読んで AL に入れる（ブロックの終わりを越えたら壊れている）
%macro UNL_GETBYTE 0
    cmp esi, ebp
    jae unl_error
    mov al, [esi]
    inc esi
%endmacro

unlzsa2:
    cld
    mov [unl_sp], esp
    mov [unl_in_end], ebx
    mov [unl_out_start], edi
    mov [unl_out_limit], edx
    mov ebp, ebx
    UNL_GETBYTE                         ; ヘッダ 7B 9E 20（20 = LZSA2）
    cmp al, 0x7B
    jne unl_error
    UNL_GETBYTE
    cmp al, 0x9E
    jne unl_error
    UNL_GETBYTE
    cmp al, 0x20
    jne unl_error

.frame:
    ; フレームの長さ: 17 ビットの大きさ + bit7 = 無圧縮。00 00 00 は終わり
    mov ebp, [unl_in_end]
    UNL_GETBYTE
    movzx ecx, al
    UNL_GETBYTE
    mov ch, al
    UNL_GETBYTE
    test al, 0x7E
    jnz unl_error
    mov bl, al
    and eax, 1
    shl eax, 16
    or ecx, eax
    lea eax, [esi + ecx]                ; フレームの終わり
    cmp eax, ebp
    ja unl_error
    test bl, 0x80
    jnz .stored
    test ecx, ecx
    jz .done
    mov ebp, eax
    mov byte [unl_nibble], 0
    xor edx, edx

.command:
    cmp esi, ebp
    jae .frame                          ; ブロックの終わり
    UNL_GETBYTE
    mov bl, al                          ; トークン XYZ|LL|MMM
    movzx ecx, al
    shr ecx, 3
    and ecx, 3
    cmp ecx, 3
    jne .literals
    call unl_getlen                     ; ECX = 3 + 続き
.literals:
    call unl_copy_literals
    cmp esi, ebp
    jae .frame                          ; 最後のコマンドはリテラルだけ

    ; オフセット（XYZ）
    mov al, bl
    and al, 0xE0
    cmp al, 0x40
    jb .off5
    cmp al, 0x80
    jb .off9
    cmp al, 0xC0
    jb .off13
    je .off16
    jmp .match_len                      ; 111 = rep-match（EDX のまま）
.off5:
    call unl_getnibble
    shl eax, 1
    test bl, 0x20
    jz .off5_z
    or al, 1
.off5_z:
    xor eax, 0x1E
    inc eax
    mov edx, eax
    jmp .match_len
.off9:
    UNL_GETBYTE
    movzx eax, al
    test bl, 0x20
    jz .off9_z
    or eax, 0x100
.off9_z:
    xor eax, 0xFF
    inc eax
    mov edx, eax
    jmp .match_len
.off13:
    call unl_getnibble
    shl eax, 9
    mov edx, eax
    UNL_GETBYTE
    mov dl, al
    test bl, 0x20
    jz .off13_z
    or edx, 0x100
.off13_z:
    xor edx, 0x1EFF
    add edx, 512 + 1
    jmp .match_len
.off16:
    UNL_GETBYTE
    movzx edx, al
    shl edx, 8
    UNL_GETBYTE
    mov dl, al
    xor edx, 0xFFFF
    inc edx

.match_len:
    movzx ecx, bl
    and ecx, 7
    cmp ecx, 7
    jne .short_match
    mov ecx, 9
    call unl_getlen
    test ecx, ecx
    jz .eod
    jmp .copy_match
.short_match:
    add ecx, 2
.copy_match:
    ; 一致: EDX バイト前から ECX バイト。出力の先頭より前は指せない
    mov eax, edi
    sub eax, [unl_out_start]
    test edx, edx
    jz unl_error
    cmp edx, eax
    ja unl_error
    mov eax, [unl_out_limit]
    sub eax, edi
    cmp ecx, eax
    ja unl_error
    push esi
    mov esi, edi
    sub esi, edx
    a32 rep movsb                       ; 前から 1 バイトずつ（重なっていてよい）
    pop esi
    jmp .command

.eod:
    ; EOD の印（raw block の終わり）。ストリームでは本来出ないが、本家と同じくブロックを終える
    mov esi, ebp
    jmp .frame

.stored:
    ; 無圧縮のフレーム
    call unl_copy_literals
    jmp .frame

.done:
    clc
    ret

; ECX バイトを ESI から EDI へ写す（入力はブロックの終わり EBP、出力は上限まで）
unl_copy_literals:
    mov eax, ebp
    sub eax, esi
    cmp ecx, eax
    ja unl_error
    mov eax, [unl_out_limit]
    sub eax, edi
    cmp ecx, eax
    ja unl_error
    a32 rep movsb
    ret

; ニブル（4 ビット）を EAX に。足りなければ 1 バイト読み、上位 4 ビットを返して下位を取っておく
; （unl_nibble の bit7 = 取っておいたものがある）
unl_getnibble:
    movzx eax, byte [unl_nibble]
    test al, al
    jz .read
    and al, 0x0F
    mov byte [unl_nibble], 0
    ret
.read:
    UNL_GETBYTE
    movzx eax, al
    push eax
    and al, 0x0F
    or al, 0x80
    mov [unl_nibble], al
    pop eax
    shr eax, 4
    ret

; 長さの続き: ECX = トークンの欄が埋まったときの値（リテラル 3、一致 9）。
; ニブル → 1 バイト → 2 バイト。1 バイトで 256 になったら EOD で ECX = 0
unl_getlen:
    call unl_getnibble
    add ecx, eax
    cmp eax, 15
    jne .done
    UNL_GETBYTE
    movzx eax, al
    add ecx, eax
    cmp ecx, 257
    je .long
    cmp ecx, 256
    jne .done
    xor ecx, ecx
.done:
    ret
.long:
    UNL_GETBYTE
    movzx ecx, al
    UNL_GETBYTE
    mov ch, al
    ret

; 壊れている: unlzsa2 を呼んだ所へ CF = 1 で戻る（途中の call の戻り先は捨てる）
unl_error:
    mov esp, [unl_sp]
    stc
    ret
