//! カーネルとアプリで共有する定義
//!
//! システムコールは `int 0x80`。EAX = 番号, EBX, ECX, EDX, ESI, EDI = 引数, 戻り値は EAX。
//! 戻り値が負ならエラー（`-エラー番号`）。

#![no_std]

/// ユーザ空間の範囲 [USER_BASE, USER_END)
pub const USER_BASE: u32 = 0x4000_0000;
pub const USER_END: u32 = 0xC000_0000;
/// ユーザスタックの上端と大きさ（その下の 1 ページはガードページ）
pub const USER_STACK_TOP: u32 = 0xBFFF_F000;
pub const USER_STACK_SIZE: u32 = 64 * 1024;

pub const SYSCALL_VECTOR: u8 = 0x80;

/// 標準入出力のファイル記述子（コンソール）
pub const STDIN: i32 = 0;
pub const STDOUT: i32 = 1;
pub const STDERR: i32 = 2;

pub mod nr {
    /// exit(code) -> !
    pub const EXIT: u32 = 1;
    /// write(fd, buf, len) -> 書いたバイト数
    pub const WRITE: u32 = 2;
    /// read(fd, buf, len) -> 読んだバイト数（0 = 終わり）。fd 0 はキーボードから 1 行
    pub const READ: u32 = 3;
    /// open(path, path_len) -> fd（読み込み専用）
    pub const OPEN: u32 = 4;
    /// close(fd) -> 0
    pub const CLOSE: u32 = 5;
    /// sbrk(増やすバイト数) -> 増やす前の終わりの番地
    pub const SBRK: u32 = 6;
    /// sleep(ms) -> 0
    pub const SLEEP: u32 = 7;
    /// ticks() -> 起動からの刻み数（TICK_HZ 回で 1 秒）
    pub const TICKS: u32 = 8;
    /// yield() -> 0
    pub const YIELD: u32 = 9;
    /// getpid() -> タスク ID
    pub const GETPID: u32 = 10;
    /// spawn(path, path_len, args, args_len) -> タスク ID。args は空白区切り
    pub const SPAWN: u32 = 11;
    /// wait(pid) -> 終了コード
    pub const WAIT: u32 = 12;
    /// stat(path, path_len, &mut Stat) -> 0
    pub const STAT: u32 = 13;
    /// readdir(path, path_len, &mut [DirEntry], count) -> 書いた数
    pub const READDIR: u32 = 14;

    /// gfx_open() -> 0。全画面の画面を作って前面に出す（終了時に自動で閉じる）
    pub const GFX_OPEN: u32 = 16;
    /// gfx_blit(buf: *const [u8; 64000]) -> 0。画面全体を書き換える
    pub const GFX_BLIT: u32 = 17;
    /// gfx_palette(start, count, rgb: *const [u8; 3]) -> 0（各 6bit）
    pub const GFX_PALETTE: u32 = 18;
    /// gfx_close() -> 0
    pub const GFX_CLOSE: u32 = 19;
    /// getkey(nonblock) -> キーイベント（`KeyEvent::encode`）。nonblock で無ければ 0
    pub const GETKEY: u32 = 20;
    /// mouse(&mut MouseState) -> 1 ならイベントあり, 0 なら無し（待たない）
    pub const MOUSE: u32 = 21;
}

pub mod err {
    pub const ENOENT: i32 = 2;
    pub const EIO: i32 = 5;
    pub const EBADF: i32 = 9;
    pub const ENOMEM: i32 = 12;
    pub const EFAULT: i32 = 14;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const EMFILE: i32 = 24;
    pub const ENOEXEC: i32 = 8;
    pub const ECHILD: i32 = 10;
    pub const ENOSYS: i32 = 38;

    pub fn name(e: i32) -> &'static str {
        match e {
            ENOENT => "not found",
            EIO => "I/O error",
            EBADF => "bad file descriptor",
            ENOMEM => "out of memory",
            EFAULT => "bad address",
            ENOTDIR => "not a directory",
            EISDIR => "is a directory",
            EINVAL => "invalid argument",
            EMFILE => "too many open files",
            ENOEXEC => "not an executable",
            ECHILD => "no such process",
            ENOSYS => "not implemented",
            _ => "error",
        }
    }
}

/// 例外で強制終了したときの終了コード
pub const EXIT_KILLED: i32 = -128;
/// Ctrl+C で止めたときの終了コード
pub const EXIT_INTERRUPTED: i32 = -130;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub size: u32,
    /// 1 ならディレクトリ
    pub is_dir: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DirEntry {
    /// "NAME.EXT"（0 で終わる）
    pub name: [u8; 16],
    pub size: u32,
    pub is_dir: u32,
}

impl DirEntry {
    pub const EMPTY: DirEntry = DirEntry { name: [0; 16], size: 0, is_dir: 0 };

    pub fn name(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    /// bit0 = 左, bit1 = 右, bit2 = 中
    pub buttons: u32,
}

/// キーイベントを 32bit に詰めたもの
/// bit 0..16 = 文字（無ければ 0）, bit 16..24 = キー（セット 1 のコード、E0 付きは 0x80 を足す）,
/// bit 24 = 押した(1)/離した(0), bit 25 = Shift, bit 26 = Ctrl, bit 27 = Alt
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent(pub u32);

impl KeyEvent {
    pub fn encode(ch: Option<char>, key: u16, pressed: bool, shift: bool, ctrl: bool, alt: bool) -> u32 {
        let c = ch.map_or(0, |c| (c as u32).min(0xFFFF));
        let k = if key & 0xE000 == 0xE000 { (key & 0x7F) | 0x80 } else { key & 0x7F } as u32;
        c | k << 16 | (pressed as u32) << 24 | (shift as u32) << 25 | (ctrl as u32) << 26 | (alt as u32) << 27
    }
    pub fn ch(self) -> Option<char> {
        char::from_u32(self.0 & 0xFFFF).filter(|&c| c != '\0')
    }
    pub fn key(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn pressed(self) -> bool {
        self.0 & (1 << 24) != 0
    }
    pub fn shift(self) -> bool {
        self.0 & (1 << 25) != 0
    }
    pub fn ctrl(self) -> bool {
        self.0 & (1 << 26) != 0
    }
    pub fn alt(self) -> bool {
        self.0 & (1 << 27) != 0
    }
}

/// よく使うキー（`KeyEvent::key` の値）
pub mod key {
    pub const ESCAPE: u8 = 0x01;
    pub const ENTER: u8 = 0x1C;
    pub const SPACE: u8 = 0x39;
    pub const UP: u8 = 0x80 | 0x48;
    pub const DOWN: u8 = 0x80 | 0x50;
    pub const LEFT: u8 = 0x80 | 0x4B;
    pub const RIGHT: u8 = 0x80 | 0x4D;
}
