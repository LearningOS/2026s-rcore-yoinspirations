//! Mutex (spin-like and blocking(sleep))

use super::deadlock::DEADLOCK_DETECTED;
use super::UPSafeCell;
use crate::task::TaskControlBlock;
use crate::task::{block_current_and_run_next, suspend_current_and_run_next};
use crate::task::{current_task, wakeup_task};
use alloc::{collections::VecDeque, sync::Arc};

/// Mutex trait
pub trait Mutex: Sync + Send {
    /// Lock the mutex
    fn lock(&self);
    /// Lock with optional deadlock detection (blocking mutex only)
    fn lock_checked(&self, _check_deadlock: bool) -> isize {
        self.lock();
        0
    }
    /// Unlock the mutex
    fn unlock(&self);
}

/// Spinlock Mutex struct
pub struct MutexSpin {
    locked: UPSafeCell<bool>,
}

impl MutexSpin {
    /// Create a new spinlock mutex
    pub fn new() -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
        }
    }
}

impl Mutex for MutexSpin {
    /// Lock the spinlock mutex
    fn lock(&self) {
        trace!("kernel: MutexSpin::lock");
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                drop(locked);
                suspend_current_and_run_next();
                continue;
            } else {
                *locked = true;
                return;
            }
        }
    }

    fn unlock(&self) {
        trace!("kernel: MutexSpin::unlock");
        let mut locked = self.locked.exclusive_access();
        *locked = false;
    }
}

/// Blocking Mutex struct
pub struct MutexBlocking {
    inner: UPSafeCell<MutexBlockingInner>,
}

pub struct MutexBlockingInner {
    locked: bool,
    owner: Option<usize>,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl MutexBlocking {
    /// Create a new blocking mutex
    pub fn new() -> Self {
        trace!("kernel: MutexBlocking::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    locked: false,
                    owner: None,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }
}

impl MutexBlocking {
    fn current_tid() -> usize {
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    }
}

impl Mutex for MutexBlocking {
    fn lock(&self) {
        assert_eq!(self.lock_checked(false), 0);
    }

    fn lock_checked(&self, check_deadlock: bool) -> isize {
        trace!("kernel: MutexBlocking::lock");
        let cur_tid = Self::current_tid();
        let mut mutex_inner = self.inner.exclusive_access();
        if mutex_inner.locked {
            if check_deadlock && mutex_inner.owner == Some(cur_tid) {
                return DEADLOCK_DETECTED;
            }
            mutex_inner.wait_queue.push_back(current_task().unwrap());
            drop(mutex_inner);
            block_current_and_run_next();
            0
        } else {
            mutex_inner.locked = true;
            mutex_inner.owner = Some(cur_tid);
            0
        }
    }

    /// unlock the blocking mutex
    fn unlock(&self) {
        trace!("kernel: MutexBlocking::unlock");
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.locked);
        if let Some(waking_task) = mutex_inner.wait_queue.pop_front() {
            mutex_inner.owner = Some(task_tid(&waking_task));
            wakeup_task(waking_task);
        } else {
            mutex_inner.locked = false;
            mutex_inner.owner = None;
        }
    }
}

fn task_tid(task: &Arc<TaskControlBlock>) -> usize {
    task.inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid
}
