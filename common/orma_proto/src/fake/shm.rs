//! A shared memory writer for tests of the reader (WP3a): it lays the file out and writes rings
//! and tables by the rules a shim follows (design 13), from Rust.
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::mem::offset_of;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};

use orma_common::events::{ORMA_EV_PAD, orma_record};
use orma_common::shm::*;
use orma_common::{Plain, bucket, bytes_of, catalog};

use crate::records;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub n_threads: u32,
    pub table_entries: u32,
    pub ring_bytes: u32,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            n_threads: ORMA_THREADS_DEFAULT,
            table_entries: ORMA_TABLE_ENTRIES_DEFAULT,
            ring_bytes: ORMA_RING_BYTES_DEFAULT,
        }
    }
}

impl Geometry {
    pub fn thread_size(&self) -> usize {
        orma_thread_size(self.table_entries, self.ring_bytes)
    }

    pub fn file_size(&self) -> usize {
        ORMA_SHM_HEADER_SIZE + self.n_threads as usize * self.thread_size()
    }
}

/// The file stays until `unlink`; dropping the writer releases its liveness lock, as a crashed
/// process does.
pub struct FakeShm {
    _file: File,
    path: PathBuf,
    map: *mut u8,
    geometry: Geometry,
}

// SAFETY: the mapping is shared memory accessed only through atomics and the ring protocol.
unsafe impl Send for FakeShm {}

fn lock(file: &File, kind: i32, byte: u32) -> io::Result<()> {
    // SAFETY: flock is plain data; zero is a valid value.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = kind as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    fl.l_start = libc::off_t::from(byte);
    fl.l_len = 1;
    // SAFETY: fl is a valid flock for the fd's lifetime of this call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &fl) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

impl FakeShm {
    pub fn create(
        dir: &Path,
        pid: u32,
        starttime: u64,
        backend: u32,
        geometry: Geometry,
    ) -> io::Result<FakeShm> {
        let suffix = if backend == ORMA_BACKEND_LTTNG {
            ".lttng"
        } else {
            ""
        };
        let path = dir.join(format!("orma.{pid}.{starttime}{suffix}"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.set_len(geometry.file_size() as u64)?;
        lock(&file, libc::F_RDLCK, ORMA_LOCK_ALIVE)?;
        // SAFETY: a fresh shared mapping of the whole file, unmapped in Drop.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                geometry.file_size(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let header = orma_shm_header {
            magic: ORMA_SHM_MAGIC,
            version: ORMA_SHM_VERSION,
            header_size: ORMA_SHM_HEADER_SIZE as u32,
            thread_size: geometry.thread_size() as u32,
            n_threads: geometry.n_threads,
            ring_bytes: geometry.ring_bytes,
            table_entries: geometry.table_entries,
            backend,
            pid,
            starttime,
            armed: 0,
            options: 0,
            reader_gen: 0,
            pad: 0,
            threads_overflow: 0,
            missing: [0; ORMA_MISSING_WORDS],
            reserved: [0; 3992],
        };
        let shm = FakeShm {
            _file: file,
            path,
            map: map.cast(),
            geometry,
        };
        shm.write_bytes(0, bytes_of(&header));
        Ok(shm)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn unlink(&self) -> io::Result<()> {
        std::fs::remove_file(&self.path)
    }

    fn write_bytes(&self, at: usize, bytes: &[u8]) {
        assert!(at + bytes.len() <= self.geometry.file_size());
        // SAFETY: in bounds of the mapping; the ring protocol keeps readers off these bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.map.wrapping_add(at), bytes.len());
        }
    }

    fn u16_at(&self, at: usize) -> &AtomicU16 {
        assert!(at.is_multiple_of(2) && at + 2 <= self.geometry.file_size());
        // SAFETY: aligned, in bounds, and only ever accessed atomically while mapped.
        unsafe { AtomicU16::from_ptr(self.map.wrapping_add(at).cast()) }
    }

    fn u32_at(&self, at: usize) -> &AtomicU32 {
        assert!(at.is_multiple_of(4) && at + 4 <= self.geometry.file_size());
        // SAFETY: as in u16_at.
        unsafe { AtomicU32::from_ptr(self.map.wrapping_add(at).cast()) }
    }

    fn u64_at(&self, at: usize) -> &AtomicU64 {
        assert!(at.is_multiple_of(8) && at + 8 <= self.geometry.file_size());
        // SAFETY: as in u16_at.
        unsafe { AtomicU64::from_ptr(self.map.wrapping_add(at).cast()) }
    }

    fn slot_at(&self, slot: usize) -> usize {
        assert!(slot < self.geometry.n_threads as usize);
        ORMA_SHM_HEADER_SIZE + slot * self.geometry.thread_size()
    }

    fn slot_field(&self, slot: usize, field: usize) -> usize {
        self.slot_at(slot) + field
    }

    pub fn armed(&self) -> bool {
        self.u32_at(offset_of!(orma_shm_header, armed))
            .load(Ordering::Relaxed)
            != 0
    }

    pub fn set_missing(&self, event: u16) {
        let word = offset_of!(orma_shm_header, missing) + (event as usize / 64) * 8;
        self.u64_at(word)
            .fetch_or(1 << (event % 64), Ordering::Relaxed);
    }

    /// A FREE slot made LIVE for `tid` (design 13.3), or `None` after counting the overflow.
    pub fn claim(&self, tid: u32) -> Option<usize> {
        for slot in 0..self.geometry.n_threads as usize {
            let state = self.u32_at(self.slot_field(slot, offset_of!(orma_thread_slot, state)));
            if state
                .compare_exchange(
                    ORMA_THREAD_FREE,
                    ORMA_THREAD_LIVE,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                let table = self.slot_at(slot) + size_of::<orma_thread_slot>();
                let table_words =
                    self.geometry.table_entries as usize * size_of::<orma_table_entry>() / 8;
                for w in 0..table_words {
                    self.u64_at(table + w * 8).store(0, Ordering::Relaxed);
                }
                for field in [
                    offset_of!(orma_thread_slot, cur_cb),
                    offset_of!(orma_thread_slot, table_overflow),
                    offset_of!(orma_thread_slot, depth_overflow),
                    offset_of!(orma_thread_slot, ring_drops),
                ] {
                    self.u64_at(self.slot_field(slot, field))
                        .store(0, Ordering::Relaxed);
                }
                self.u32_at(self.slot_field(slot, offset_of!(orma_thread_slot, tid)))
                    .store(tid, Ordering::Relaxed);
                self.u32_at(self.slot_field(slot, offset_of!(orma_thread_slot, generation)))
                    .fetch_add(1, Ordering::Release);
                return Some(slot);
            }
        }
        self.u64_at(offset_of!(orma_shm_header, threads_overflow))
            .fetch_add(1, Ordering::Relaxed);
        None
    }

    pub fn exit(&self, slot: usize) {
        self.u32_at(self.slot_field(slot, offset_of!(orma_thread_slot, state)))
            .store(ORMA_THREAD_EXITED, Ordering::Release);
    }

    pub fn set_current(&self, slot: usize, cb: u64, src: u16, entry: u32) {
        self.u32_at(self.slot_field(slot, offset_of!(orma_thread_slot, cur_src)))
            .store(u32::from(src), Ordering::Relaxed);
        self.u32_at(self.slot_field(slot, offset_of!(orma_thread_slot, cur_entry)))
            .store(entry, Ordering::Relaxed);
        self.u64_at(self.slot_field(slot, offset_of!(orma_thread_slot, cur_cb)))
            .store(cb, Ordering::Relaxed);
    }

    /// Adds one sample to the row of `(kind, src, entry, key)`, probing from the shim's hash;
    /// false if the table is full.
    pub fn add(&self, slot: usize, kind: u16, src: u16, entry: u32, key: u64, value: u64) -> bool {
        let table = self.slot_at(slot) + size_of::<orma_thread_slot>();
        let row_size = size_of::<orma_table_entry>();
        let n = self.geometry.table_entries as usize;
        let mix =
            (key ^ (key >> 17) ^ (u64::from(entry) << 7) ^ u64::from(kind) ^ (u64::from(src) << 3))
                .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let start = (mix >> 32) as usize % n;
        for i in (0..n).map(|p| (start + p) % n) {
            let row = table + i * row_size;
            let kind_word = self.u16_at(row + offset_of!(orma_table_entry, kind));
            let found = match kind_word.load(Ordering::Acquire) {
                0 => {
                    self.u16_at(row + offset_of!(orma_table_entry, src))
                        .store(src, Ordering::Relaxed);
                    self.u32_at(row + offset_of!(orma_table_entry, entry))
                        .store(entry, Ordering::Relaxed);
                    self.u64_at(row + offset_of!(orma_table_entry, key))
                        .store(key, Ordering::Relaxed);
                    kind_word.store(kind, Ordering::Release);
                    true
                }
                k => {
                    k == kind
                        && self
                            .u16_at(row + offset_of!(orma_table_entry, src))
                            .load(Ordering::Relaxed)
                            == src
                        && self
                            .u32_at(row + offset_of!(orma_table_entry, entry))
                            .load(Ordering::Relaxed)
                            == entry
                        && self
                            .u64_at(row + offset_of!(orma_table_entry, key))
                            .load(Ordering::Relaxed)
                            == key
                }
            };
            if found {
                let at = |f: usize| self.u64_at(row + f);
                at(offset_of!(orma_table_entry, count)).fetch_add(1, Ordering::Relaxed);
                at(offset_of!(orma_table_entry, sum)).fetch_add(value, Ordering::Relaxed);
                at(offset_of!(orma_table_entry, max)).fetch_max(value, Ordering::Relaxed);
                at(offset_of!(orma_table_entry, hist) + bucket(value) * 8)
                    .fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        self.u64_at(self.slot_field(slot, offset_of!(orma_thread_slot, table_overflow)))
            .fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Counts a callback that started beyond the shim's nesting depth.
    pub fn overflow_depth(&self, slot: usize) {
        self.u64_at(self.slot_field(slot, offset_of!(orma_thread_slot, depth_overflow)))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Appends a record to the slot's ring when `armed`, or always for an init event, as the shim
    /// does. Returns false when it counted a drop or wrote nothing.
    pub fn push<T: Plain>(
        &self,
        slot: usize,
        event: u16,
        t_mono_ns: u64,
        payload: &T,
        strings: &[&[u8]],
    ) -> bool {
        if !self.armed() && !catalog::init(event) {
            return false;
        }
        let mut record = Vec::new();
        records::encode(&mut record, event, t_mono_ns, payload, strings);
        let ring = self.slot_at(slot) + orma_ring_offset(self.geometry.table_entries);
        let size = u64::from(self.geometry.ring_bytes);
        let head_word = self.u64_at(self.slot_field(slot, offset_of!(orma_thread_slot, ring_head)));
        let head = head_word.load(Ordering::Relaxed);
        let tail = self
            .u64_at(self.slot_field(slot, offset_of!(orma_thread_slot, ring_tail)))
            .load(Ordering::Acquire);
        let pos = head % size;
        let to_end = size - pos;
        let len = record.len() as u64;
        let need = if len <= to_end { len } else { to_end + len };
        if size - (head - tail) < need {
            self.u64_at(self.slot_field(slot, offset_of!(orma_thread_slot, ring_drops)))
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let mut at = pos;
        if len > to_end {
            let pad = orma_record {
                event: ORMA_EV_PAD,
                len: to_end as u16,
                flags: 0,
                t_mono_ns,
            };
            let pad = bytes_of(&pad);
            self.write_bytes(ring + pos as usize, &pad[..pad.len().min(to_end as usize)]);
            at = 0;
        }
        self.write_bytes(ring + at as usize, &record);
        head_word.store(head + need, Ordering::Release);
        true
    }
}

impl Drop for FakeShm {
    fn drop(&mut self) {
        // SAFETY: the mapping made in create, not used after this.
        unsafe { libc::munmap(self.map.cast(), self.geometry.file_size()) };
    }
}
