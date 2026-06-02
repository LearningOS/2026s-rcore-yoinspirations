//! Implementation of [`TaskManager`]
//!
//! It is only used to manage processes and schedule process based on ready queue.
//! Other CPU process monitoring functions are in Processor.

use super::{TaskControlBlock, TaskStatus};
use crate::config::BIG_STRIDE;
use crate::sync::UPSafeCell;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use lazy_static::*;

pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
    /// round-robin cursor when multiple tasks share the same stride
    rr_cursor: usize,
}

/// Stride scheduler: pick the task with minimum stride.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
            rr_cursor: 0,
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Take the ready task with the smallest stride
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let len = self.ready_queue.len();
        if len == 0 {
            return None;
        }
        let mut min_stride = usize::MAX;
        for task in self.ready_queue.iter() {
            let inner = task.inner_exclusive_access();
            if inner.task_status == TaskStatus::Ready && inner.stride < min_stride {
                min_stride = inner.stride;
            }
        }
        // Among tasks with minimum stride, pick in round-robin order.
        for offset in 0..len {
            let idx = (self.rr_cursor + offset) % len;
            let pick = {
                let task = &self.ready_queue[idx];
                let inner = task.inner_exclusive_access();
                inner.task_status == TaskStatus::Ready && inner.stride == min_stride
            };
            if pick {
                self.rr_cursor = (idx + 1) % len;
                let task = self.ready_queue.remove(idx).unwrap();
                {
                    let mut inner = task.inner_exclusive_access();
                    inner.stride = inner
                        .stride
                        .saturating_add(BIG_STRIDE / inner.priority.max(2));
                }
                return Some(task);
            }
        }
        None
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
    /// PID2PCB instance (map of pid to pcb)
    pub static ref PID2TCB: UPSafeCell<BTreeMap<usize, Arc<TaskControlBlock>>> =
        unsafe { UPSafeCell::new(BTreeMap::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    PID2TCB
        .exclusive_access()
        .insert(task.getpid(), Arc::clone(&task));
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.exclusive_access().fetch()
}

/// Get process by pid
pub fn pid2task(pid: usize) -> Option<Arc<TaskControlBlock>> {
    let map = PID2TCB.exclusive_access();
    map.get(&pid).map(Arc::clone)
}

/// Remove item(pid, _some_pcb) from PDI2PCB map (called by exit_current_and_run_next)
pub fn remove_from_pid2task(pid: usize) {
    let mut map = PID2TCB.exclusive_access();
    if map.remove(&pid).is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
}
