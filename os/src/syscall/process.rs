//! Process management syscalls
use crate::config::{MAXVA, PAGE_SIZE};
use crate::mm::{translated_byte_buffer, MapPermission, PageTable, PTEFlags, VirtAddr, VPNRange};
use crate::task::{
    change_program_brk, current_map_area, current_translate, current_unmap_area,
    current_user_token, exit_current_and_run_next, get_syscall_times, suspend_current_and_run_next,
};
use crate::timer::get_time_us;

const TRACE_READ: usize = 0;
const TRACE_WRITE: usize = 1;
const TRACE_SYSCALL: usize = 2;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

fn user_byte_readable(token: usize, addr: usize) -> bool {
    let vpn = VirtAddr::from(addr).floor();
    PageTable::from_token(token)
        .translate(vpn)
        .map(|pte| pte.is_valid() && pte.readable() && pte.flags().contains(PTEFlags::U))
        .unwrap_or(false)
}

fn user_byte_writable(token: usize, addr: usize) -> bool {
    let vpn = VirtAddr::from(addr).floor();
    PageTable::from_token(token)
        .translate(vpn)
        .map(|pte| pte.is_valid() && pte.writable() && pte.flags().contains(PTEFlags::U))
        .unwrap_or(false)
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
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

/// trace syscall: read/write user memory with permission check, or query syscall count
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    let token = current_user_token();
    match trace_request {
        TRACE_READ => {
            if !user_byte_readable(token, id) {
                return -1;
            }
            let buffers = translated_byte_buffer(token, id as *const u8, 1);
            if buffers.is_empty() {
                -1
            } else {
                buffers[0][0] as isize
            }
        }
        TRACE_WRITE => {
            if !user_byte_writable(token, id) {
                return -1;
            }
            let mut buffers = translated_byte_buffer(token, id as *const u8, 1);
            if buffers.is_empty() {
                -1
            } else {
                buffers[0][0] = data as u8;
                0
            }
        }
        TRACE_SYSCALL => get_syscall_times(id) as isize,
        _ => -1,
    }
}

/// map framed pages into current task address space
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel: sys_mmap");
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
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        if let Some(pte) = current_translate(vpn) {
            if pte.is_valid() {
                return -1;
            }
        }
    }
    let map_perm =
        MapPermission::from_bits_truncate((prot << 1) as u8) | MapPermission::U;
    current_map_area(start_vpn.into(), end_vpn.into(), map_perm);
    0
}

/// unmap pages in current task address space
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    if start >= MAXVA || start % PAGE_SIZE != 0 {
        return -1;
    }
    let mut mlen = len;
    if start > MAXVA - len {
        mlen = MAXVA - start;
    }
    if current_unmap_area(start, mlen) {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
