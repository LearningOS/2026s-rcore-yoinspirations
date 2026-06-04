//!Implementation of [`TaskManager`]
use super::{TaskControlBlock, TaskStatus};
use crate::config::BIG_STRIDE;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
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
            let take = {
                let task = &self.ready_queue[idx];
                let inner = task.inner_exclusive_access();
                inner.task_status == TaskStatus::Ready && inner.stride == min_stride
            };
            if take {
                self.rr_cursor = (idx + 1) % len;
                let task = self.ready_queue.remove(idx).unwrap();
                {
                    let mut inner = task.inner_exclusive_access();
                    let new_stride = inner
                        .stride
                        .saturating_add(BIG_STRIDE / inner.priority.max(2));
                    inner.stride = new_stride;
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
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
