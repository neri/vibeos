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
use crate::fs::TaskFs;
use crate::mm::{paging, pmm};
use crate::sync::{IrqMutex, Mutex};

pub type TaskId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Blocked,
    Dead,
}

const STACK_PAGES: usize = 4; // 16KB
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
}

pub struct TaskInfo {
    pub id: TaskId,
    pub name: String,
    pub state: State,
    pub ticks: u32,
}

struct Scheduler {
    started: bool,
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
                fs: TaskFs::new_child(&[]),
            }),
        );
        s.current = id;
    }
    let idle = create("idle", Box::new(idle_loop), false);
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

    /// 終了したタスクのスタックを解放する（実行中のタスク自身のものは除く）
    fn reap(&mut self) {
        let mut i = 0;
        while i < self.dead.len() {
            let id = self.dead[i];
            if id == self.current {
                i += 1;
                continue;
            }
            self.dead.swap_remove(i);
            self.tasks.remove(&id);
        }
    }
}

type Entry = Box<dyn FnOnce() + Send + 'static>;

/// カーネルスレッドを作って実行待ちにする
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> TaskId {
    create(name, Box::new(f), true)
}

fn create(name: &str, entry: Entry, make_ready: bool) -> TaskId {
    let stack = KernelStack::new().expect("sched: no memory for a kernel stack");
    let arg = Box::into_raw(Box::new(entry)) as u32;
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
    // カレントディレクトリは作ったタスクから受け継ぐ（ロックの順番を守るため SCHED の外で読む）
    let parent_cwd: Vec<String> = if is_started() { current_fs().lock().cwd_parts().to_vec() } else { Vec::new() };
    let fs = TaskFs::new_child(&parent_cwd);
    let mut s = SCHED.lock();
    let id = s.alloc_id();
    s.tasks.insert(
        id,
        Box::new(Task {
            id,
            name: String::from(name),
            state: if make_ready { State::Ready } else { State::Blocked },
            esp: sp as u32,
            stack: Some(stack),
            ticks: 0,
            fs,
        }),
    );
    if make_ready {
        s.ready.push_back(id);
    }
    id
}

extern "C" fn task_start(arg: u32) -> ! {
    // SAFETY: 新しいタスクは割り込み可能な状態で始める
    unsafe { cpu::enable_interrupts() };
    // SAFETY: create が Box::into_raw で渡したもの
    let entry = unsafe { Box::from_raw(arg as *mut Entry) };
    entry();
    exit()
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
    let switch = {
        let mut s = SCHED.lock();
        s.reap();
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
            if let Some(stack) = &n.stack {
                gdt::set_kernel_stack(stack.top() as u32);
            }
            let old_esp = &mut s.task(cur).esp as *mut u32;
            Some((old_esp, new_esp))
        }
    };
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

/// 今のタスクを終える
pub fn exit() -> ! {
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
        .map(|t| TaskInfo { id: t.id, name: t.name.clone(), state: t.state, ticks: t.ticks })
        .collect()
}

/// アイドルタスクが動いていた刻み数
#[allow(dead_code)]
pub fn idle_ticks() -> u32 {
    let mut s = SCHED.lock();
    let idle = s.idle;
    s.task(idle).ticks
}
