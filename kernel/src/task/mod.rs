//! タスク管理（DESIGN.md §6.4）

pub mod sched;
pub mod wait;

#[allow(unused_imports)]
pub use sched::{TaskId, current_id, exit, join, spawn, yield_now};
