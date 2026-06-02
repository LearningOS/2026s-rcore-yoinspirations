//! Process management syscalls
//!
use alloc::sync::Arc;

use crate::{
    config::{MAXVA, PAGE_SIZE},
    fs::{open_file, OpenFlags},
    mm::{
        translated_byte_buffer, translated_refmut, translated_str, MapPermission, VirtAddr,
        VPNRange,
    },
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    trap_cx.x[10] = 0;
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, block and retry.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!("kernel::pid[{}] sys_waitpid [{}]", current_task().unwrap().pid.0, pid);
    loop {
        let task = current_task().unwrap();
        let token = task.get_user_token();
        let mut inner = task.inner_exclusive_access();
        if !inner
            .children
            .iter()
            .any(|p| pid == -1 || pid as usize == p.getpid())
        {
            return -1;
        }
        let pair = inner.children.iter().enumerate().find(|(_, p)| {
            p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        });
        if let Some((idx, _)) = pair {
            let child = inner.children.remove(idx);
            assert_eq!(Arc::strong_count(&child), 1);
            let found_pid = child.getpid();
            let exit_code = child.inner_exclusive_access().exit_code;
            *translated_refmut(token, exit_code_ptr) = exit_code;
            return found_pid as isize;
        }
        drop(inner);
        suspend_current_and_run_next();
    }
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel:pid[{}] sys_get_time", current_task().unwrap().pid.0);
    let us = get_time_us();
    let time_val = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let buffers = translated_byte_buffer(
        current_user_token(),
        ts as *const u8,
        core::mem::size_of::<TimeVal>(),
    );
    let src_ptr = &time_val as *const TimeVal as *const u8;
    let mut offset = 0usize;
    for buffer in buffers {
        let len = buffer.len();
        unsafe {
            buffer.copy_from_slice(core::slice::from_raw_parts(src_ptr.add(offset), len));
        }
        offset += len;
    }
    0
}

/// map framed pages into current task address space
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap", current_task().unwrap().pid.0);
    if start % PAGE_SIZE != 0
        || prot & !0x7 != 0
        || prot & 0x7 == 0
        || start >= MAXVA
        || start.checked_add(len).is_none()
        || start + len > MAXVA
    {
        return -1;
    }
    let start_vpn = VirtAddr::from(start).floor();
    let end_vpn = VirtAddr::from(start + len).ceil();
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        if let Some(pte) = inner.memory_set.translate(vpn) {
            if pte.is_valid() {
                return -1;
            }
        }
    }
    let map_perm = MapPermission::from_bits_truncate((prot << 1) as u8) | MapPermission::U;
    inner
        .memory_set
        .insert_framed_area(start_vpn.into(), end_vpn.into(), map_perm);
    0
}

/// unmap pages in current task address space
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap", current_task().unwrap().pid.0);
    if start >= MAXVA || start % PAGE_SIZE != 0 {
        return -1;
    }
    let mut mlen = len;
    if start > MAXVA - len {
        mlen = MAXVA - start;
    }
    let start_vpn = VirtAddr::from(start).floor();
    let end_vpn = VirtAddr::from(start + mlen).ceil();
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        match inner.memory_set.translate(vpn) {
            Some(pte) if pte.is_valid() => {
                inner.memory_set.page_table_mut().unmap(vpn);
            }
            _ => return -1,
        }
    }
    0
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// create a child process that runs the given program (not fork+exec in parent)
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) else {
        return -1;
    };
    let all_data = app_inode.read_all();
    let task = current_task().unwrap();
    let child = task.spawn(all_data.as_slice());
    let pid = child.pid.0 as isize;
    add_task(child);
    pid
}

/// set stride scheduling priority (must be > 1)
pub fn sys_set_priority(prio: isize) -> isize {
    trace!("kernel:pid[{}] sys_set_priority", current_task().unwrap().pid.0);
    if prio <= 1 {
        return -1;
    }
    let task = current_task().unwrap();
    task.inner_exclusive_access().set_priority(prio as usize);
    prio
}
