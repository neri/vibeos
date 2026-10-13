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

/// ticks() の 1 秒あたりの刻み数
pub const TICK_HZ: u32 = 100;

/// open のフラグ
pub const OPEN_WRITE: u32 = 1;
/// 無ければ作る
pub const OPEN_CREATE: u32 = 2;
/// 0 バイトにしてから書く
pub const OPEN_TRUNCATE: u32 = 4;
/// いつも終わりに書き足す
pub const OPEN_APPEND: u32 = 8;

/// gfx_open のフラグ: マウスカーソルを表示する
pub const GFX_CURSOR: u32 = 1;

/// win_open のフラグ: 中身は 32 ビット（0x00RRGGBB）。win_blit は幅 × 高さ × 4 バイトを読む。
/// 320x200 の全画面では標準のパレットの近い色で出る
pub const WIN_RGB32: u32 = 1;

/// font_data の種類: UI の文字・太字・固定幅（`.VFN`。vibe_font::vfn）
pub const FONT_UI: u32 = 0;
pub const FONT_BOLD: u32 = 1;
pub const FONT_MONO: u32 = 2;
/// icon_data のアイコンの 1 辺
pub const ICON_SIZE: usize = 16;

/// 画面の大きさ（モード 13h）
pub const SCREEN_WIDTH: usize = 320;
pub const SCREEN_HEIGHT: usize = 200;

/// 標準入出力のファイル記述子（何もつなぎ替えなければ端末）
pub const STDIN: i32 = 0;
pub const STDOUT: i32 = 1;
pub const STDERR: i32 = 2;

pub mod nr {
    /// exit(code) -> !
    pub const EXIT: u32 = 1;
    /// write(fd, buf, len) -> 書いたバイト数。読む側のいないパイプに書くとプロセスは EXIT_PIPE で止まる
    pub const WRITE: u32 = 2;
    /// read(fd, buf, len) -> 読んだバイト数（0 = 終わり）。端末からはキーボードから編集付きで 1 行
    pub const READ: u32 = 3;
    /// open(path, path_len, flags) -> fd。flags は OPEN_*（0 なら読み込み専用）
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
    /// spawn(path, path_len, args, args_len) -> タスク ID。args は空白区切り。環境は空、
    /// 標準入出力（fd 0〜2）は親のものを受け継ぐ
    pub const SPAWN: u32 = 11;
    /// wait(pid) -> 終了コード
    pub const WAIT: u32 = 12;
    /// stat(path, path_len, &mut Stat) -> 0
    pub const STAT: u32 = 13;
    /// readdir(path, path_len, &mut [DirEntry], count) -> 書いた数
    pub const READDIR: u32 = 14;
    /// mkdir(path, path_len) -> 0
    pub const MKDIR: u32 = 15;
    /// remove(path, path_len) -> 0（ファイルか空のディレクトリ）
    pub const REMOVE: u32 = 22;
    /// sync() -> 書き戻したセクタ数。RAM ディスクの変更をフロッピーに書き戻す
    pub const SYNC: u32 = 23;
    /// sound(freq, ms) -> 0。PC スピーカーで鳴らす（待たない）。freq 0 で止める
    pub const SOUND: u32 = 24;
    /// time() -> 1970 年からの秒（現地時刻）。時計が無ければ 0
    pub const TIME: u32 = 25;
    /// win_open(w, h, title, title_len, flags) -> ウィンドウ番号。デスクトップでは枠付きのウィンドウ、
    /// 全画面のときは前面に出る。flags は WIN_*（WIN_RGB32 なら中身は 32 ビット）
    pub const WIN_OPEN: u32 = 26;
    /// win_blit(win, buf) -> 0。中身全体を書き換える（8 ビットなら w*h バイト、WIN_RGB32 なら w*h*4 バイト）
    pub const WIN_BLIT: u32 = 27;
    /// win_event(win, &mut WinEvent, timeout_ms) -> 1 ならイベントあり, 0 なら時間切れ。
    /// timeout_ms = 0 なら待たない、u32::MAX ならいつまでも待つ
    pub const WIN_EVENT: u32 = 28;
    /// win_close(win) -> 0
    pub const WIN_CLOSE: u32 = 29;

    /// socket(kind) -> fd。kind は SOCK_*。read / write / close はファイルと同じ番号で使える
    pub const SOCKET: u32 = 30;
    /// bind(fd, port) -> 0。自分のポートを決める（UDP の受け口・TCP の待ち受けの前）
    pub const BIND: u32 = 31;
    /// connect(fd, &SockAddr) -> 0。TCP でつながるまで待つ
    pub const CONNECT: u32 = 32;
    /// listen(fd, backlog) -> 0。bind したポートで TCP の接続を待ち受ける
    pub const LISTEN: u32 = 33;
    /// accept(fd, &mut SockAddr) -> 新しい fd。つながってくるまで待つ
    pub const ACCEPT: u32 = 34;
    /// sendto(fd, buf, len, &SockAddr) -> 送ったバイト数。ICMP では buf はエコー要求のメッセージ全体
    /// （識別子とチェックサムはカーネルが入れる）。TCP では宛先を見ない
    pub const SENDTO: u32 = 35;
    /// recvfrom(fd, buf, len, &mut SockAddr, timeout_ms) -> 受け取ったバイト数（入りきらない分は捨てる）。
    /// timeout_ms = 0 なら待たない、u32::MAX ならいつまでも。時間切れは -ETIMEDOUT。TCP の 0 は終わり
    pub const RECVFROM: u32 = 36;
    /// net_info(&mut NetInfo) -> 0。カードが無ければ -ENODEV
    pub const NET_INFO: u32 = 37;
    /// poll(&mut [PollFd], n, timeout_ms) -> 準備のできた数（0 なら時間切れ）
    pub const POLL: u32 = 38;
    /// ctrl_c(op) -> 値。op = CTRL_C_CATCH: Ctrl+C でプロセスを止めず印を付ける（待っているシステムコールは
    /// -EINTR で戻る）。CTRL_C_TAKE: 印があれば 1 を返して消す。CTRL_C_DEFAULT: 止める（既定）に戻す
    pub const CTRL_C: u32 = 39;
    /// set_time(secs) -> 0。時計を 1970 年からの秒（現地時刻）に合わせ、CMOS にも書く
    pub const SET_TIME: u32 = 40;
    /// term_size() -> (桁 << 16) | 行。プロセスがつながっている端末の大きさ。端末が無ければ -ENOTTY
    pub const TERM_SIZE: u32 = 41;
    /// rename(from, from_len, to, to_len) -> 0。名前を変える・移す（to が既にあるディレクトリならその中へ）
    pub const RENAME: u32 = 42;
    /// detach(pid) -> 0。子の終わるのを待たない（終了コードを残さない）
    pub const DETACH: u32 = 43;
    /// term_open(cmd, cmd_len) -> 端末の番号。新しい端末を開く。cmd が空でなければ、そのコマンドを
    /// 実行して終わったら端末を閉じる（空なら対話のシェル）
    pub const TERM_OPEN: u32 = 44;
    /// proc_list(&mut [ProcInfo], n) -> 書いた数。タスク（カーネルスレッドとプロセス）の一覧（番号の順）
    pub const PROC_LIST: u32 = 45;
    /// kill(pid) -> 0。プロセスを止める（カーネルスレッドは -EPERM、無ければ -ECHILD）
    pub const KILL: u32 = 46;
    /// sys_info(&mut SysInfo) -> 0。メモリ・時間・画面の情報
    pub const SYS_INFO: u32 = 47;
    /// win_palette(win, start, count, rgb: *const [u8; 3]) -> 0。ウィンドウ独自のパレット（各 6bit）。
    /// デスクトップでは標準のパレットの近い色に置き換えて出る
    pub const WIN_PALETTE: u32 = 48;
    /// clip_set(buf, len) -> 0。クリップボードに文字を入れる（4096 バイトまで）
    pub const CLIP_SET: u32 = 49;
    /// clip_get(buf, len) -> クリップボードの文字の長さ（buf には入るだけ写す）
    pub const CLIP_GET: u32 = 50;
    /// pipe(&mut [i32; 2]) -> 0。[0] が読む側、[1] が書く側の fd
    pub const PIPE: u32 = 51;
    /// dup2(old, new) -> new。new が開いていれば閉じてから、old と同じものを指すようにする
    pub const DUP2: u32 = 52;
    /// fd_kind(fd) -> FD_*（開いていなければ -EBADF）
    pub const FD_KIND: u32 = 53;
    /// spawn_ex(&SpawnArgs) -> タスク ID。引数・環境・標準入出力を指定して起動する
    pub const SPAWN_EX: u32 = 54;
    /// fs_info(&mut FsInfo) -> 0。ディスクの空きと、まだ書き戻していないセクタの数
    pub const FS_INFO: u32 = 55;
    /// pci_list(&mut [PciInfo], n) -> 書いた数。PCI の機器の一覧（PCI が無ければ -ENODEV）
    pub const PCI_LIST: u32 = 56;
    /// arp_list(&mut [ArpEntry], n) -> 書いた数。ARP の表
    pub const ARP_LIST: u32 = 57;
    /// arp_resolve(ip: [u8; 4] を詰めた u32, timeout_ms, &mut [u8; 6]) -> 0。ARP で MAC を引く
    pub const ARP_RESOLVE: u32 = 58;
    /// sock_list(&mut [SockInfo], n) -> 書いた数。ソケットの一覧（全プロセスの分）
    pub const SOCK_LIST: u32 = 59;
    /// chdir(path, path_len) -> 0。今のプロセスのカレントディレクトリを変える（子は起動したときのものを受け継ぐ）
    pub const CHDIR: u32 = 60;
    /// getcwd(buf, len) -> カレントディレクトリの長さ（buf には入るだけ写す）
    pub const GETCWD: u32 = 61;
    /// wait_opt(pid, flags, &mut code) -> 終わったタスクの ID（WAIT_NOHANG で、まだ終わっていなければ 0）。
    /// pid = -1 なら自分が起動したどれか 1 つ。待つものが無ければ -ECHILD
    pub const WAIT_OPT: u32 = 62;
    /// set_foreground(&[pid], n) -> 0。今の端末の前面のプロセスの集まり（Ctrl+C で止める相手。n = 0 で無し）
    pub const SET_FOREGROUND: u32 = 63;
    // 64 は S1〜S2 のつなぎ（kcmd: カーネルの内蔵コマンドを呼ぶ）に使っていた
    /// reboot() -> 戻らない。計算機を起動し直す
    pub const REBOOT: u32 = 65;
    /// wallpaper_size() -> (幅 << 16) | 高さ。デスクトップの背景の画像の大きさ（デスクトップでなければ -EPERM）
    pub const WALLPAPER_SIZE: u32 = 66;
    /// wallpaper_set(pixels, len) -> 0。背景の画像（標準のパレットの番号、幅 × 高さ）。len = 0 なら単色に戻す
    pub const WALLPAPER_SET: u32 = 67;
    /// cmos_time(&mut [u8; 6]) -> 1970 年からの秒。CMOS の時計を読み直す。読めない値なら -EIO
    /// （buf には読んだままの 秒・分・時・日・月・年）
    pub const CMOS_TIME: u32 = 68;
    /// demo() -> 0。3 つのカーネルスレッドが端末の右上に数を描く（5 秒。動いている間は -EAGAIN）
    pub const DEMO: u32 = 69;
    /// fault_test(kind) -> 戻らない。カーネルの中でわざと例外を起こす（FAULT_* 。テスト用のカーネルだけ。
    /// ほかは -ENOSYS）
    pub const FAULT_TEST: u32 = 70;
    /// font_data(kind, buf, len) -> フォントの大きさ（バイト）。kind は FONT_*。len がそれ以上なら buf に
    /// 写す（足りなければ写さずに大きさだけ返す）。カーネルが起動時に読んだ `/SYSTEM/FONTS` の `.VFN`
    /// （展開したもの）。無ければ -ENOENT
    pub const FONT_DATA: u32 = 71;
    /// icon_data(name, name_len, buf: *mut [u32; 256]) -> 0。16x16 のアイコン（事前に掛けたアルファの
    /// 0xAARRGGBB）。無ければ -ENOENT
    pub const ICON_DATA: u32 = 72;

    /// gfx_open(flags) -> 0。全画面の画面を作って前面に出す（終了時に自動で閉じる）。flags は GFX_*
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
    pub const EINTR: i32 = 4;
    pub const EAGAIN: i32 = 11;
    pub const ENODEV: i32 = 19;
    pub const ENOTSOCK: i32 = 88;
    pub const EMSGSIZE: i32 = 90;
    pub const EADDRINUSE: i32 = 98;
    pub const ENETDOWN: i32 = 100;
    pub const ENETUNREACH: i32 = 101;
    pub const ECONNRESET: i32 = 104;
    pub const ENOTCONN: i32 = 107;
    pub const ETIMEDOUT: i32 = 110;
    pub const ECONNREFUSED: i32 = 111;
    pub const EHOSTUNREACH: i32 = 113;
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
    pub const EEXIST: i32 = 17;
    pub const ENOSPC: i32 = 28;
    pub const EROFS: i32 = 30;
    pub const ENOTEMPTY: i32 = 39;
    pub const ENOTTY: i32 = 25;
    pub const EPERM: i32 = 1;
    pub const EPIPE: i32 = 32;
    pub const E2BIG: i32 = 7;

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
            EEXIST => "already exists",
            ENOSPC => "no space left",
            EROFS => "read-only",
            ENOTEMPTY => "directory not empty",
            ENOTTY => "no terminal",
            EPERM => "not permitted",
            EINTR => "interrupted",
            EPIPE => "broken pipe",
            E2BIG => "argument list too long",
            EAGAIN => "try again",
            ENODEV => "no network card",
            ENOTSOCK => "not a socket",
            EMSGSIZE => "message too long",
            EADDRINUSE => "address in use",
            ENETDOWN => "network not configured",
            ENETUNREACH => "no route to host",
            ECONNRESET => "connection reset",
            ENOTCONN => "not connected",
            ETIMEDOUT => "timed out",
            ECONNREFUSED => "connection refused",
            EHOSTUNREACH => "host unreachable",
            _ => "error",
        }
    }
}

/// fault_test の種類
pub const FAULT_DIVZERO: u32 = 1;
pub const FAULT_NULL: u32 = 2;
pub const FAULT_STACK: u32 = 3;
pub const FAULT_PANIC: u32 = 4;

/// wait_opt のフラグ: 終わっていなければ待たずに 0 を返す
pub const WAIT_NOHANG: u32 = 1;

/// 例外で強制終了したときの終了コード
pub const EXIT_KILLED: i32 = -128;
/// Ctrl+C で止めたときの終了コード
pub const EXIT_INTERRUPTED: i32 = -130;
/// 読む側のいないパイプに書いて止められたときの終了コード（Unix の SIGPIPE と同じ数）
pub const EXIT_PIPE: i32 = -141;

/// fd_kind の結果
pub const FD_CONSOLE: u32 = 1;
pub const FD_FILE: u32 = 2;
pub const FD_PIPE: u32 = 3;
pub const FD_SOCKET: u32 = 4;

/// spawn_ex で渡せる引数と環境の合計の上限（文字列と区切りの 0）
pub const MAX_ARG_BYTES: usize = 16 * 1024;

/// spawn_ex の引数
///
/// `argv` は引数を 0 で区切って並べたもの（各引数の後ろに 0。`argv[0]` はプログラムの名前）、
/// `envp` は `名前=値` を同じように並べたもの。`fds` は子の fd 0〜2 にする親の fd（-1 なら親の同じ番号）。
/// `path` は名前だけなら `/BIN/名前.ELF`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SpawnArgs {
    pub path: u32,
    pub path_len: u32,
    pub argv: u32,
    pub argv_len: u32,
    pub envp: u32,
    pub envp_len: u32,
    pub fds: [i32; 3],
}

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
    /// 書き換えた日付と時刻（FAT の形。`vibe_time::DateTime::from_fat`）
    pub date: u16,
    pub time: u16,
}

impl DirEntry {
    pub const EMPTY: DirEntry = DirEntry { name: [0; 16], size: 0, is_dir: 0, date: 0, time: 0 };

    pub fn name(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }
}

/// ウィンドウのイベント（win_event）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinEvent {
    /// WIN_EVENT_*
    pub kind: u32,
    /// キーのとき: `KeyEvent` に詰めた値
    pub key: u32,
    /// マウスのとき: ウィンドウの中身の座標とボタン（bit0 = 左, bit1 = 右, bit2 = 中）
    pub x: i32,
    pub y: i32,
    pub buttons: u32,
}

pub const WIN_EVENT_KEY: u32 = 1;
/// マウスが動いた・ボタンが変わった。マウスがウィンドウから出たときは x = y = -1（ボタンなし）
pub const WIN_EVENT_MOUSE: u32 = 2;
/// [x] が押された（閉じるかどうかはアプリが決める）
pub const WIN_EVENT_CLOSE: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    /// bit0 = 左, bit1 = 右, bit2 = 中
    pub buttons: u32,
}

/// タスクの情報（proc_list）
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProcInfo {
    pub id: u32,
    /// 動いていたタイマの刻み数
    pub ticks: u32,
    /// 使っているメモリ（ユーザ空間のページとカーネルスタック）
    pub mem_kb: u32,
    /// PROC_*
    pub state: u32,
    /// PROC_USER など
    pub flags: u32,
    /// つながっている端末
    pub tty: u32,
    /// 名前（0 で終わる）
    pub name: [u8; 16],
}

pub const PROC_READY: u32 = 0;
pub const PROC_RUNNING: u32 = 1;
pub const PROC_BLOCKED: u32 = 2;
pub const PROC_DEAD: u32 = 3;
/// ユーザプロセス（そうでなければカーネルスレッド）
pub const PROC_USER: u32 = 1;

impl ProcInfo {
    pub const EMPTY: ProcInfo = ProcInfo { id: 0, ticks: 0, mem_kb: 0, state: 0, flags: 0, tty: 0, name: [0; 16] };

    pub fn name(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }
}

/// システムの情報（sys_info）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SysInfo {
    pub free_kb: u32,
    pub total_kb: u32,
    /// 起動からの刻み数と、そのうちアイドルだった刻み数
    pub ticks: u32,
    pub idle_ticks: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    /// SYS_DESKTOP など
    pub flags: u32,
    /// カーネルのヒープ（空きバイト数, 全体のバイト数, 空きのかたまりの数）
    pub heap_free: u32,
    pub heap_total: u32,
    pub heap_blocks: u32,
}

/// ディスクの情報（fs_info）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FsInfo {
    pub free_bytes: u32,
    pub total_bytes: u32,
    /// RAM ディスクで変えたが、まだフロッピーに書き戻していないセクタの数
    pub dirty_sectors: u32,
}

/// PCI の機器（pci_list）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PciInfo {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub class: u8,
    pub subclass: u8,
    /// 割り込みの線（0 か 0xFF なら無し）
    pub irq: u8,
    pub _pad: u16,
    pub vendor: u16,
    pub device: u16,
    /// BAR 0 が I/O ならその番地、メモリならその番地（どちらかだけ。無ければ 0）
    pub io: u32,
    pub mem: u32,
}

/// PCI の機器の種類の名前
pub fn pci_class_name(class: u8, subclass: u8) -> &'static str {
    match (class, subclass) {
        (0x01, 0x01) => "IDE controller",
        (0x01, _) => "storage",
        (0x02, 0x00) => "Ethernet",
        (0x02, _) => "network",
        (0x03, _) => "display",
        (0x04, _) => "multimedia",
        (0x05, _) => "memory",
        (0x06, 0x00) => "host bridge",
        (0x06, 0x01) => "ISA bridge",
        (0x06, 0x04) => "PCI bridge",
        (0x06, _) => "bridge",
        (0x0C, 0x03) => "USB",
        (0x0C, _) => "serial bus",
        _ => "other",
    }
}

/// ARP の表の 1 件（arp_list）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArpEntry {
    pub ip: [u8; 4],
    pub mac: [u8; 6],
    pub _pad: u16,
}

/// ソケットの 1 件（sock_list）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SockInfo {
    /// "tcp" など（0 で終わる）
    pub kind: [u8; 8],
    /// "ESTABLISHED" など（0 で終わる）
    pub state: [u8; 16],
    pub local_port: u16,
    pub remote_port: u16,
    /// 相手（無ければ 0.0.0.0）
    pub remote: [u8; 4],
    pub recv_queued: u32,
    pub send_queued: u32,
}

/// 0 で終わる固定長の名前を文字列に
pub fn fixed_str(bytes: &[u8]) -> &str {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).unwrap_or("?")
}

/// 文字列を 0 で終わる固定長に入れる（入りきらない分は捨てる。最後の 1 バイトは 0 のまま）
pub fn set_fixed_str(out: &mut [u8], s: &str) {
    let n = s.len().min(out.len().saturating_sub(1));
    out[..n].copy_from_slice(&s.as_bytes()[..n]);
}

/// 画面がデスクトップ（ウィンドウを重ねて出す）
pub const SYS_DESKTOP: u32 = 1;

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
    pub const BACKSPACE: u8 = 0x0E;
    pub const TAB: u8 = 0x0F;
    pub const ENTER: u8 = 0x1C;
    pub const SPACE: u8 = 0x39;
    pub const HOME: u8 = 0x80 | 0x47;
    pub const PAGE_UP: u8 = 0x80 | 0x49;
    pub const END: u8 = 0x80 | 0x4F;
    pub const PAGE_DOWN: u8 = 0x80 | 0x51;
    pub const DELETE: u8 = 0x80 | 0x53;
    pub const INSERT: u8 = 0x80 | 0x52;
    pub const UP: u8 = 0x80 | 0x48;
    pub const DOWN: u8 = 0x80 | 0x50;
    pub const LEFT: u8 = 0x80 | 0x4B;
    pub const RIGHT: u8 = 0x80 | 0x4D;
}

/// socket の種類
pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
/// ICMP のエコー（ping）
pub const SOCK_ICMP: u32 = 3;

/// ctrl_c の操作
pub const CTRL_C_DEFAULT: u32 = 0;
pub const CTRL_C_CATCH: u32 = 1;
pub const CTRL_C_TAKE: u32 = 2;

/// ソケットのアドレス（IPv4 とポート）
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SockAddr {
    /// ネットワークのバイト順（10.0.2.15 なら [10, 0, 2, 15]）
    pub addr: [u8; 4],
    pub port: u16,
    pub _pad: u16,
}

impl SockAddr {
    pub fn new(addr: [u8; 4], port: u16) -> SockAddr {
        SockAddr { addr, port, _pad: 0 }
    }
}

/// net_info の結果
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NetInfo {
    pub mac: [u8; 6],
    /// NET_CONFIGURED など
    pub flags: u16,
    pub addr: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    /// BOOT.CFG の ntp=（無ければ 0.0.0.0）
    pub ntp: [u8; 4],
    /// BOOT.CFG の tz=（UTC との差, 分）
    pub tz_minutes: i32,
    pub rx_packets: u32,
    pub tx_packets: u32,
    pub rx_bytes: u32,
    pub tx_bytes: u32,
    /// 受け取れずに捨てたフレームの数
    pub rx_dropped: u32,
    /// カードの名前（0 で終わる）
    pub card: [u8; 16],
    /// カードの I/O ポートと IRQ
    pub io: u16,
    pub irq: u8,
    pub _pad: u8,
}

/// アドレスが決まっている
pub const NET_CONFIGURED: u16 = 1;
/// DHCP を使っている
pub const NET_DHCP: u16 = 2;

/// poll の 1 件
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollFd {
    pub fd: i32,
    /// 待つもの（POLL_IN / POLL_OUT）
    pub events: u16,
    /// 準備のできたもの（カーネルが書く）
    pub revents: u16,
}

/// 読める（データ・接続の要求・終わり・エラー）
pub const POLL_IN: u16 = 1;
/// 書ける
pub const POLL_OUT: u16 = 4;
