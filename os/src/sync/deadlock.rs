//! Deadlock detection (banker's algorithm for semaphores, self-lock for mutex)

use crate::sync::Semaphore;
use crate::task::TaskControlBlock;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

/// Return value when a syscall is rejected due to deadlock
pub const DEADLOCK_DETECTED: isize = -0xdead as isize;

fn task_tid(task: &Arc<TaskControlBlock>) -> usize {
    task.inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid
}

fn task_exit_code(task: &Arc<TaskControlBlock>) -> Option<i32> {
    task.inner_exclusive_access().exit_code
}

/// Banker's safety check before `semaphore_down(sem_id)` by thread `tid`.
pub fn sem_down_would_deadlock(
    tasks: &[Option<Arc<TaskControlBlock>>],
    semaphores: &[Option<Arc<Semaphore>>],
    allocation: &[Vec<usize>],
    tid: usize,
    sem_id: usize,
) -> bool {
    let n = tasks.len();
    let m = semaphores.len();
    if sem_id >= m {
        return false;
    }
    if semaphores[sem_id].is_none() {
        return false;
    }

    let mut work = vec![0isize; m];
    for j in 0..m {
        if let Some(sem) = &semaphores[j] {
            work[j] = sem.inner.exclusive_access().count.max(0);
        }
    }

    let mut alloc = vec![vec![0isize; m]; n];
    for i in 0..n {
        if i < allocation.len() {
            for j in 0..m {
                alloc[i][j] = allocation[i][j] as isize;
            }
        }
    }

    let mut need = vec![vec![0isize; m]; n];
    for j in 0..m {
        if let Some(sem) = &semaphores[j] {
            let inner = sem.inner.exclusive_access();
            for task in inner.wait_queue.iter() {
                let t = task_tid(task);
                if t < n {
                    need[t][j] += 1;
                }
            }
        }
    }

    // Request cannot be satisfied immediately (caller checks count <= 0).
    if tid < n {
        need[tid][sem_id] += 1;
    }

    let mut finish = vec![true; n];
    for i in 0..n {
        if let Some(task) = &tasks[i] {
            if task_exit_code(task).is_none() {
                finish[i] = false;
            }
        }
    }

    loop {
        let mut found = false;
        for i in 0..n {
            if finish[i] || tasks[i].is_none() {
                continue;
            }
            if (0..m).all(|j| need[i][j] <= work[j]) {
                finish[i] = true;
                found = true;
                for j in 0..m {
                    work[j] += alloc[i][j];
                }
            }
        }
        if !found {
            break;
        }
    }

    for i in 0..n {
        if let Some(task) = &tasks[i] {
            if task_exit_code(task).is_none() && !finish[i] {
                return true;
            }
        }
    }
    false
}
