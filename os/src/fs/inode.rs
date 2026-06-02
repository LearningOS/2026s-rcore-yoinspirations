//! `Arc<Inode>` -> `OSInodeInner`: In order to open files concurrently
//! we need to wrap `Inode` into `Arc`,but `Mutex` in `Inode` prevents
//! file systems from being accessed simultaneously
//!
//! `UPSafeCell<OSInodeInner>` -> `OSInode`: for static `ROOT_INODE`,we
//! need to wrap `OSInodeInner` into `UPSafeCell`
use super::{File, Stat, StatMode};
use crate::drivers::BLOCK_DEVICE;
use crate::mm::UserBuffer;
use crate::sync::UPSafeCell;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::*;
use easy_fs::{EasyFileSystem, Inode};
use lazy_static::*;

/// inode in memory
/// A wrapper around a filesystem inode
/// to implement File trait atop
pub struct OSInode {
    readable: bool,
    writable: bool,
    inner: UPSafeCell<OSInodeInner>,
}
/// The OS inode inner in 'UPSafeCell'
pub struct OSInodeInner {
    offset: usize,
    inode: Arc<Inode>,
    canonical_name: String,
}

#[derive(Default)]
struct LinkState {
    // alias path -> canonical path
    aliases: BTreeMap<String, String>,
    // canonical path -> hard link count
    nlinks: BTreeMap<String, u32>,
    // canonical path hidden by unlink (inode may still exist physically)
    tombstones: BTreeSet<String>,
}

lazy_static! {
    static ref LINK_STATE: UPSafeCell<LinkState> = unsafe { UPSafeCell::new(LinkState::default()) };
}

impl OSInode {
    /// create a new inode in memory
    pub fn new(readable: bool, writable: bool, inode: Arc<Inode>, canonical_name: String) -> Self {
        Self {
            readable,
            writable,
            inner: unsafe {
                UPSafeCell::new(OSInodeInner {
                    offset: 0,
                    inode,
                    canonical_name,
                })
            },
        }
    }
    /// read all data from the inode
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.exclusive_access();
        let mut buffer: Vec<u8> = Vec::with_capacity(512);
        buffer.resize(512, 0);
        let mut v: Vec<u8> = Vec::new();
        loop {
            let len = inner.inode.read_at(inner.offset, &mut buffer);
            if len == 0 {
                break;
            }
            inner.offset += len;
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }
}

lazy_static! {
    pub static ref ROOT_INODE: Arc<Inode> = {
        let efs = EasyFileSystem::open(BLOCK_DEVICE.clone());
        Arc::new(EasyFileSystem::root_inode(&efs))
    };
}

/// List all apps in the root directory
pub fn list_apps() {
    println!("/**** APPS ****");
    for app in ROOT_INODE.ls() {
        println!("{}", app);
    }
    println!("**************/");
}

bitflags! {
    ///  The flags argument to the open() system call is constructed by ORing together zero or more of the following values:
    pub struct OpenFlags: u32 {
        /// readyonly
        const RDONLY = 0;
        /// writeonly
        const WRONLY = 1 << 0;
        /// read and write
        const RDWR = 1 << 1;
        /// create new file
        const CREATE = 1 << 9;
        /// truncate file size to 0
        const TRUNC = 1 << 10;
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
}

/// Open a file
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
    let (readable, writable) = flags.read_write();
    let mut state = LINK_STATE.exclusive_access();
    let canonical = resolve_name_in(&state, name);
    // Tombstone hides the canonical path only; aliases may still open the inode.
    if !flags.contains(OpenFlags::CREATE)
        && state.tombstones.contains(canonical.as_str())
        && !state.aliases.contains_key(name)
    {
        return None;
    }
    if flags.contains(OpenFlags::CREATE) {
        state.tombstones.remove(canonical.as_str());
        if let Some(inode) = ROOT_INODE.find(canonical.as_str()) {
            // clear size
            inode.clear();
            state.nlinks.entry(canonical.clone()).or_insert(1);
            Some(Arc::new(OSInode::new(
                readable,
                writable,
                inode,
                canonical.clone(),
            )))
        } else {
            // create file
            ROOT_INODE
                .create(canonical.as_str())
                .map(|inode| {
                    state.nlinks.insert(canonical.clone(), 1);
                    Arc::new(OSInode::new(readable, writable, inode, canonical.clone()))
                })
        }
    } else {
        ROOT_INODE.find(canonical.as_str()).map(|inode| {
            if flags.contains(OpenFlags::TRUNC) {
                inode.clear();
            }
            state.nlinks.entry(canonical.clone()).or_insert(1);
            Arc::new(OSInode::new(readable, writable, inode, canonical.clone()))
        })
    }
}

fn resolve_name_in(state: &LinkState, name: &str) -> String {
    let mut cur = String::from(name);
    while let Some(next) = state.aliases.get(cur.as_str()) {
        cur = next.clone();
    }
    cur
}

fn resolve_name(name: &str) -> String {
    let state = LINK_STATE.exclusive_access();
    resolve_name_in(&state, name)
}

/// Create a hard-link-like alias from `new_name` to `old_name`.
pub fn link_file(old_name: &str, new_name: &str) -> isize {
    let canonical = resolve_name(old_name);
    if ROOT_INODE.find(canonical.as_str()).is_none() {
        return -1;
    }
    let mut state = LINK_STATE.exclusive_access();
    if state.tombstones.contains(canonical.as_str())
        || state.aliases.contains_key(new_name)
        || state.tombstones.contains(new_name)
        || ROOT_INODE.find(new_name).is_some()
    {
        return -1;
    }
    state.aliases.insert(String::from(new_name), canonical.clone());
    *state.nlinks.entry(canonical).or_insert(1) += 1;
    0
}

/// Remove a path name from root directory link state.
pub fn unlink_file(name: &str) -> isize {
    let mut state = LINK_STATE.exclusive_access();
    if let Some(canonical) = state.aliases.remove(name) {
        if let Some(cnt) = state.nlinks.get_mut(canonical.as_str()) {
            if *cnt > 1 {
                *cnt -= 1;
            }
        }
        return 0;
    }
    let canonical = resolve_name_in(&state, name);
    if ROOT_INODE.find(canonical.as_str()).is_none() || state.tombstones.contains(canonical.as_str()) {
        return -1;
    }
    state.tombstones.insert(canonical.clone());
    let cnt = state.nlinks.entry(canonical).or_insert(1);
    if *cnt > 1 {
        *cnt -= 1;
    }
    0
}

fn inode_nlink(canonical_name: &str) -> u32 {
    // Since easy-fs in this lab does not persist hard links in on-disk metadata,
    // keep hard-link counts in kernel memory by canonical path.
    let state = LINK_STATE.exclusive_access();
    // If not found, default to regular single-link files.
    state.nlinks.get(canonical_name).copied().unwrap_or(1)
}

impl File for OSInode {
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn read(&self, mut buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_read_size = 0usize;
        for slice in buf.buffers.iter_mut() {
            let read_size = inner.inode.read_at(inner.offset, *slice);
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read_size += read_size;
        }
        total_read_size
    }
    fn write(&self, buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_write_size = 0usize;
        for slice in buf.buffers.iter() {
            let write_size = inner.inode.write_at(inner.offset, *slice);
            assert_eq!(write_size, slice.len());
            inner.offset += write_size;
            total_write_size += write_size;
        }
        total_write_size
    }
    fn fstat(&self) -> Stat {
        let inner = self.inner.exclusive_access();
        Stat {
            dev: 0,
            ino: 0,
            mode: StatMode::FILE,
            nlink: inode_nlink(inner.canonical_name.as_str()),
            pad: [0; 7],
        }
    }
}
