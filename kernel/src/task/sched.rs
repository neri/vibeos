//! スケジューラ（ラウンドロビン、プリエンプティブ）
//!
//! - 各タスクは専用のカーネルスタック（16KB + その下にガードページ）を持つ
//! - 切り替えは `switch_context` 1 本: 呼び出し規約で保存すべきレジスタを積んで ESP を入れ替える。
//!   タイマ割り込みからの切り替えも、割り込みハンドラの中でこれを呼ぶ（割り込みの戻り先は
//!   各タスクのスタックに残っているので、戻ってきたときにそのまま iret できる）
//! - タイマの刻み QUANTUM 回ごとに、実行待ちのタスクがあれば切り替える
//! - 実行できるタスクが無いときはアイドルタスクが `hlt` で待つ
//!
//! ロックの順番: WaitQueue → SCHED → HEAP → FRAMES
//! `switch_context` を呼ぶときはどのロックも持っていないこと（割り込みは禁止のまま）。

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::global_asm;

use vibe_alloc::frame::PAGE_SIZE;

use super::wait::WaitQueue;
use crate::arch::{cpu, gdt};
use crate::fs::{Stdio, TaskFs};
use crate::mm::{paging, pmm};
use crate::process::UserRef;
use crate::sync::{IrqMutex, Mutex};

pub type TaskId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Blocked,
    Dead,
}

pub const STACK_PAGES: usize = 4; // 16KB
const QUANTUM: u32 = 2; // 20ms
const MAX_TASKS_HINT: usize = 64;

/// タスクごとのカーネルスタック。一番下のページは対応付けを外したガードページ
struct KernelStack {
    base: usize,
}

impl KernelStack {
    fn new() -> Option<KernelStack> {
        let base = pmm::alloc_pages(STACK_PAGES + 1)?;
        paging::unmap(paging::kernel_page_directory(), base);
        Some(KernelStack { base })
    }

    fn top(&self) -> usize {
        self.base + (STACK_PAGES + 1) * PAGE_SIZE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        // ガードページを元に戻してから物理ページを返す
        paging::map(paging::kernel_page_directory(), self.base, self.base, paging::WRITABLE)
            .expect("restore guard page");
        pmm::free_pages(self.base, STACK_PAGES + 1);
    }
}

pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub state: State,
    /// 切り替えで止まっている間の ESP
    esp: u32,
    /// 起動時のタスク（ブートスタックを使う）は None
    stack: Option<KernelStack>,
    /// このタスクが実行中だったタイマの刻み数
    pub ticks: u32,
    /// 開いているファイル・カレントディレクトリ
    fs: Arc<Mutex<TaskFs>>,
    /// このタスクのページディレクトリ（カーネルスレッドはカーネルのもの）
    cr3: usize,
    /// ユーザプロセスならそのアドレス空間など
    user: Option<UserRef>,
    /// つながっている端末（端末のウィンドウの番号）。標準入出力とキーはここへ
    tty: u32,
    /// 作ったタスク（wait_opt の「自分の子」）
    parent: TaskId,
}

pub struct TaskInfo {
    pub id: TaskId,
    pub name: String,
    pub state: State,
    pub ticks: u32,
    pub tty: u32,
    /// ユーザプロセスならそのアドレス空間など
    pub user: Option<UserRef>,
}

struct Scheduler {
    started: bool,
    /// 今 CR3 に入っているページディレクトリ
    active_cr3: usize,
    tasks: BTreeMap<TaskId, Box<Task>>,
    ready: VecDeque<TaskId>,
    current: TaskId,
    idle: TaskId,
    dead: Vec<TaskId>,
    next_id: TaskId,
    slice_left: u32,
}

static SCHED: IrqMutex<Scheduler> = IrqMutex::new(Scheduler {
    started: false,
    active_cr3: 0,
    tasks: BTreeMap::new(),
    ready: VecDeque::new(),
    current: 0,
    idle: 0,
    dead: Vec::new(),
    next_id: 0,
    slice_left: QUANTUM,
});

/// タスクが終わるたびに起こされる（join 用）
static EXIT_WAIT: WaitQueue = WaitQueue::new();

global_asm!(
    ".global switch_context",
    "switch_context:", // (old_esp: *mut u32, new_esp: u32)
    "    push ebp",
    "    push ebx",
    "    push esi",
    "    push edi",
    "    mov eax, [esp + 20]",
    "    mov [eax], esp",
    "    mov esp, [esp + 24]",
    "    pop edi",
    "    pop esi",
    "    pop ebx",
    "    pop ebp",
    "    ret",
);

unsafe extern "C" {
    fn switch_context(old_esp: *mut u32, new_esp: u32);
}

/// 今動いている処理（kmain）を最初のタスクとして登録し、アイドルタスクを作る
pub fn init() {
    {
        let mut s = SCHED.lock();
        s.ready.reserve(MAX_TASKS_HINT);
        s.dead.reserve(MAX_TASKS_HINT);
        let id = s.alloc_id();
        s.tasks.insert(
            id,
            Box::new(Task {
                id,
                name: String::from("boot"),
                state: State::Running,
                esp: 0,
                stack: None,
                ticks: 0,
                fs: TaskFs::new_root(),
                cr3: paging::kernel_page_directory(),
                user: None,
                tty: 0,
                parent: id,
            }),
        );
        s.current = id;
        s.active_cr3 = paging::kernel_page_directory();
    }
    let idle = create("idle", Start::Kernel(Box::new(idle_loop)), false, None, None);
    let mut s = SCHED.lock();
    s.idle = idle;
    s.started = true;
    klog!("sched: started");
}

pub fn is_started() -> bool {
    SCHED.lock().started
}

impl Scheduler {
    fn alloc_id(&mut self) -> TaskId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn task(&mut self, id: TaskId) -> &mut Task {
        self.tasks.get_mut(&id).expect("sched: no such task")
    }

    /// 終了したタスクを表から外して返す（実行中のタスク自身は除く）。返したものは SCHED を
    /// 手放してから捨てること（開いていたソケットを閉じるときに他のタスクを起こすため）
    fn reap(&mut self) -> Vec<Box<Task>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.dead.len() {
            let id = self.dead[i];
            if id == self.current {
                i += 1;
                continue;
            }
            self.dead.swap_remove(i);
            if let Some(t) = self.tasks.remove(&id) {
                out.push(t);
            }
        }
        out
    }
}

type Entry = Box<dyn FnOnce() + Send + 'static>;

/// タスクの始め方
enum Start {
    /// カーネルスレッド: 関数を呼ぶ
    Kernel(Entry),
    /// ユーザプロセス: (入口, ユーザスタック) で ring 3 に入る
    User(u32, u32),
}

/// カーネルスレッドを作って実行待ちにする
/// （fd 0〜2 は作ったタスクのものを受け継ぐ）
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> TaskId {
    create(name, Start::Kernel(Box::new(f)), true, None, None)
}

/// カーネルスレッドを、fd 0〜2 を `stdio` にして作る
pub fn spawn_with_stdio(name: &str, stdio: Stdio, f: impl FnOnce() + Send + 'static) -> TaskId {
    create(name, Start::Kernel(Box::new(f)), true, None, Some(stdio))
}

/// ユーザプロセスのタスクを作る（そのアドレス空間で `entry` から ring 3 で始まる）。
/// `stdio` が None なら fd 0〜2 は作ったタスクのものを受け継ぐ
pub fn spawn_user(name: &str, user: UserRef, entry: u32, user_esp: u32, stdio: Option<Stdio>) -> TaskId {
    create(name, Start::User(entry, user_esp), true, Some(user), stdio)
}

fn create(name: &str, start: Start, make_ready: bool, user: Option<UserRef>, stdio: Option<Stdio>) -> TaskId {
    let stack = KernelStack::new().expect("sched: no memory for a kernel stack");
    let arg = Box::into_raw(Box::new(start)) as u32;
    // switch_context が pop して ret したときに task_start(arg) が呼ばれるよう積んでおく。
    // 関数の入口で ESP+4 が 16 の倍数になるよう（データレイアウトの S128）、上端に 3 語の詰め物を置く
    let mut sp = stack.top() as *mut u32;
    let initial = [
        0,
        0,
        0,
        arg,
        0, // task_start の戻り先（戻らないので使わない）
        task_start as *const () as u32,
        0, // ebp
        0, // ebx
        0, // esi
        0, // edi
    ];
    for v in initial {
        // SAFETY: 確保したばかりのスタックの上端から積む
        unsafe {
            sp = sp.sub(1);
            sp.write(v);
        }
    }
    // カレントディレクトリと fd 0〜2 は作ったタスクから受け継ぐ（ロックの順番を守るため SCHED の外で読む）
    let fs = {
        let parent = current_fs();
        let p = parent.lock();
        TaskFs::new_child(p.cwd_parts(), stdio.unwrap_or_else(|| p.stdio()))
    };
    // 端末も作ったタスクから受け継ぐ
    let tty = current_tty();
    let cr3 = match &user {
        Some(u) => u.lock().space.page_directory(),
        None => paging::kernel_page_directory(),
    };
    let mut s = SCHED.lock();
    let id = s.alloc_id();
    let parent = s.current;
    s.tasks.insert(
        id,
        Box::new(Task {
            parent,
            id,
            name: String::from(name),
            state: if make_ready { State::Ready } else { State::Blocked },
            esp: sp as u32,
            stack: Some(stack),
            ticks: 0,
            fs,
            cr3,
            user,
            tty,
        }),
    );
    if make_ready {
        s.ready.push_back(id);
    }
    id
}

extern "C" fn task_start(arg: u32) -> ! {
    // SAFETY: create が Box::into_raw で渡したもの（ここで取り出して Box は解放する）
    let start = *unsafe { Box::from_raw(arg as *mut Start) };
    match start {
        Start::Kernel(entry) => {
            // SAFETY: 新しいタスクは割り込み可能な状態で始める
            unsafe { cpu::enable_interrupts() };
            entry();
            exit()
        }
        // SAFETY: このタスクのアドレス空間にプログラムとスタックが用意されている（process::spawn）。
        // ユーザモードには戻らない関数で入るので、ここでヒープのものを持っていないこと
        Start::User(entry, esp) => unsafe { crate::arch::usermode::enter(entry, esp) },
    }
}

fn idle_loop() {
    loop {
        cpu::disable_interrupts();
        if SCHED.lock().ready.is_empty() {
            // sti の直後の hlt までは割り込みが入らないので、確認してから止まるまでの間の起床を取りこぼさない
            // SAFETY: 割り込みを許可して止まる
            unsafe { core::arch::asm!("sti", "hlt", options(nomem, nostack)) };
        } else {
            // SAFETY: アイドルタスクは割り込み可能な状態で動く
            unsafe { cpu::enable_interrupts() };
            yield_now();
        }
    }
}

/// 次に動かすタスクを選んで切り替える
fn schedule() {
    let was_enabled = cpu::interrupts_enabled();
    cpu::disable_interrupts();
    let reaped;
    let switch = {
        let mut s = SCHED.lock();
        reaped = s.reap();
        let cur = s.current;
        let cur_running = s.task(cur).state == State::Running;
        let next = match s.ready.pop_front() {
            Some(id) => id,
            None if cur_running => cur,
            None => s.idle,
        };
        if next == cur {
            None
        } else {
            if cur_running {
                s.task(cur).state = State::Ready;
                if cur != s.idle {
                    s.ready.push_back(cur);
                }
            }
            s.current = next;
            s.slice_left = QUANTUM;
            let n = s.task(next);
            n.state = State::Running;
            let new_esp = n.esp;
            let new_cr3 = n.cr3;
            if let Some(stack) = &n.stack {
                gdt::set_kernel_stack(stack.top() as u32);
            }
            if new_cr3 != s.active_cr3 {
                // カーネル空間は全アドレス空間で共有しているので、ここで切り替えても続けて動ける
                // SAFETY: new_cr3 は有効なページディレクトリ
                unsafe { core::arch::asm!("mov cr3, {}", in(reg) new_cr3, options(nostack, preserves_flags)) };
                s.active_cr3 = new_cr3;
            }
            let old_esp = &mut s.task(cur).esp as *mut u32;
            Some((old_esp, new_esp))
        }
    };
    // 終わったタスクのスタック・ファイル（ソケット）などを片付ける（割り込みは禁止のまま）
    drop(reaped);
    if let Some((old, new)) = switch {
        // SAFETY: ロックは手放し、割り込みは禁止のまま。old は Box に入ったタスクの中なので切り替え中も動かない
        unsafe { switch_context(old, new) };
    }
    if was_enabled {
        // SAFETY: 呼ばれたときは許可されていた
        unsafe { cpu::enable_interrupts() };
    }
}

/// 他の実行待ちのタスクに順番を譲る
pub fn yield_now() {
    schedule();
}

/// 今のタスクを待ち状態にして切り替える。割り込み禁止で呼ぶこと（WaitQueue から使う）
pub fn block_current() {
    debug_assert!(!cpu::interrupts_enabled());
    {
        let mut s = SCHED.lock();
        let cur = s.current;
        s.task(cur).state = State::Blocked;
    }
    schedule();
}

/// 待ち状態のタスクを実行待ちにする（割り込みハンドラからも呼んでよい）
pub fn wake(id: TaskId) {
    let mut s = SCHED.lock();
    if let Some(t) = s.tasks.get_mut(&id) {
        if t.state == State::Blocked {
            t.state = State::Ready;
            s.ready.push_back(id);
        }
    }
}

/// 今のタスクを終える。開いているファイル（パイプ）はここで閉じる（読む側に終わりが届くように）
pub fn exit() -> ! {
    crate::fs::close_all();
    crate::process::forget_requests(current_id());
    cpu::disable_interrupts();
    {
        let mut s = SCHED.lock();
        let cur = s.current;
        s.task(cur).state = State::Dead;
        s.dead.push(cur);
    }
    EXIT_WAIT.notify_all();
    schedule();
    unreachable!("a dead task was scheduled");
}

pub fn current_id() -> TaskId {
    SCHED.lock().current
}

/// 今のタスクがユーザプロセスなら、そのアドレス空間など
pub fn current_user() -> Option<UserRef> {
    let mut s = SCHED.lock();
    let cur = s.current;
    s.task(cur).user.clone()
}

pub fn current_name() -> String {
    let mut s = SCHED.lock();
    let cur = s.current;
    s.task(cur).name.clone()
}

/// 今のタスクの端末（スケジューラが動く前は最初の端末）
pub fn current_tty() -> u32 {
    let s = SCHED.lock();
    s.tasks.get(&s.current).map_or(0, |t| t.tty)
}

/// 今のタスクの端末を変える（新しい端末のシェルが始めに呼ぶ）
pub fn set_current_tty(tty: u32) {
    let mut s = SCHED.lock();
    let cur = s.current;
    s.task(cur).tty = tty;
}

/// ユーザプロセスか（割り込みの中から呼んでよい）
pub fn is_user(id: TaskId) -> bool {
    SCHED.lock().tasks.get(&id).is_some_and(|t| t.user.is_some())
}

/// タスクがまだある（終わって回収されていない）か
pub fn exists(id: TaskId) -> bool {
    SCHED.lock().tasks.contains_key(&id)
}

/// 今のタスクのファイルシステムの状態
pub fn current_fs() -> Arc<Mutex<TaskFs>> {
    let mut s = SCHED.lock();
    let cur = s.current;
    s.task(cur).fs.clone()
}

fn is_alive(id: TaskId) -> bool {
    SCHED.lock().tasks.get(&id).is_some_and(|t| t.state != State::Dead)
}

/// タスクが終わるまで待つ
pub fn join(id: TaskId) {
    EXIT_WAIT.wait_until(|| !is_alive(id));
}

/// どれかのタスクが終わるたびに `cond` を見直し、true になるまで待つ
pub fn wait_exit_until(cond: impl FnMut() -> bool) {
    EXIT_WAIT.wait_until(cond);
}

/// 今のタスクを作ったタスク
pub fn current_parent() -> TaskId {
    let mut s = SCHED.lock();
    let cur = s.current;
    s.task(cur).parent
}

/// `roots` の子孫（まだ動いているユーザプロセス。`roots` 自身は除く）を `out` に入れて数を返す。
/// `skip` が true のタスク（裏で動かしているもの）とその子孫は入れない。割り込みの中から呼んでよい
pub fn descendants(roots: &[TaskId], out: &mut [TaskId], mut skip: impl FnMut(TaskId) -> bool) -> usize {
    let s = SCHED.lock();
    let mut n = 0;
    for t in s.tasks.values() {
        if n == out.len() {
            break;
        }
        if t.user.is_none() || t.state == State::Dead || roots.contains(&t.id) {
            continue;
        }
        // 親をたどって roots に着くか（深さは 16 まで）
        let mut id = t.id;
        for _ in 0..16 {
            if skip(id) {
                break;
            }
            let Some(p) = s.tasks.get(&id).map(|t| t.parent).filter(|&p| p != id) else { break };
            if roots.contains(&p) {
                out[n] = t.id;
                n += 1;
                break;
            }
            id = p;
        }
    }
    n
}

/// `parent` が作った、まだ動いているユーザプロセスがあるか
pub fn has_live_child(parent: TaskId) -> bool {
    SCHED.lock().tasks.values().any(|t| t.parent == parent && t.user.is_some() && t.state != State::Dead)
}

/// タイマ割り込みから呼ばれる。持ち時間を使い切ったら切り替える
pub fn on_tick() {
    let preempt = {
        let mut s = SCHED.lock();
        if !s.started {
            return;
        }
        let cur = s.current;
        s.task(cur).ticks += 1;
        s.slice_left = s.slice_left.saturating_sub(1);
        !s.ready.is_empty() && (s.slice_left == 0 || cur == s.idle)
    };
    if preempt {
        schedule();
    }
}

/// タスクの一覧
pub fn tasks() -> Vec<TaskInfo> {
    SCHED
        .lock()
        .tasks
        .values()
        .map(|t| TaskInfo {
            id: t.id,
            name: t.name.clone(),
            state: t.state,
            ticks: t.ticks,
            tty: t.tty,
            user: t.user.clone(),
        })
        .collect()
}

/// アイドルタスクが動いていた刻み数
pub fn idle_ticks() -> u32 {
    let mut s = SCHED.lock();
    let idle = s.idle;
    s.task(idle).ticks
}
