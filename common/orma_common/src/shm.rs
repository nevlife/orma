//! The LD_PRELOAD shared memory file `/dev/shm/orma.<pid>.<starttime>` (design 13). cbindgen
//! writes `orma_preload/include/orma_preload/orma_shm.h` from this file, so the names are the C
//! names.
#![allow(non_camel_case_types)]

/// "ORMASHM\0" read as a little-endian u64.
pub const ORMA_SHM_MAGIC: u64 = 0x004D_4853_414D_524F;
pub const ORMA_SHM_VERSION: u32 = 1;
pub const ORMA_SHM_HEADER_SIZE: usize = 4096;
pub const ORMA_CACHE_LINE: usize = 64;

/// log2 buckets over nanoseconds: bucket `i` holds `[2^i, 2^(i+1))`, bucket 0 also holds 0.
pub const ORMA_BUCKETS: usize = 40;
pub const ORMA_MISSING_WORDS: usize = 4;

pub const ORMA_THREADS_DEFAULT: u32 = 256;
pub const ORMA_TABLE_ENTRIES_DEFAULT: u32 = 128;
pub const ORMA_RING_BYTES_DEFAULT: u32 = 262144;

pub const ORMA_BACKEND_SHM: u32 = 1;
pub const ORMA_BACKEND_LTTNG: u32 = 2;

pub const ORMA_OPTION_MALLOC: u32 = 1;
/// CUDA driver calls (design 20.1).
pub const ORMA_OPTION_GPU: u32 = 2;
/// DDS write calls (design 20.2).
pub const ORMA_OPTION_DDS: u32 = 4;

/// Byte offsets of the OFD locks: the writer holds a read lock on the first while it lives, a
/// reader a write lock on the second while it reads.
pub const ORMA_LOCK_ALIVE: u32 = 0;
pub const ORMA_LOCK_READER: u32 = 1;

pub const ORMA_THREAD_FREE: u32 = 0;
pub const ORMA_THREAD_LIVE: u32 = 1;
pub const ORMA_THREAD_EXITED: u32 = 2;

/// What a callback key belongs to: a ROS callback handle, or a flux executor with its entry.
pub const ORMA_SRC_ROS: u16 = 1;
pub const ORMA_SRC_FLUX: u16 = 2;

/// Table rows. Intervals sum nanoseconds; `ORMA_AGG_ALLOC` and `ORMA_AGG_FREE` sum bytes.
pub const ORMA_AGG_CALLBACK: u16 = 1;
pub const ORMA_AGG_DISPATCH: u16 = 2;
pub const ORMA_AGG_WAIT: u16 = 3;
pub const ORMA_AGG_OFFCPU: u16 = 4;
pub const ORMA_AGG_CPU_WAIT: u16 = 5;
pub const ORMA_AGG_ALLOC: u16 = 6;
pub const ORMA_AGG_FREE: u16 = 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct orma_shm_header {
    pub magic: u64,
    pub version: u32,
    pub header_size: u32,
    pub thread_size: u32,
    pub n_threads: u32,
    pub ring_bytes: u32,
    pub table_entries: u32,
    pub backend: u32,
    pub pid: u32,
    pub starttime: u64,
    pub armed: u32,
    pub options: u32,
    pub reader_gen: u32,
    pub pad: u32,
    pub threads_overflow: u64,
    /// Bit `n` set: the intercept point of event `n` found no original function.
    pub missing: [u64; ORMA_MISSING_WORDS],
    pub reserved: [u8; 3992],
}

/// The fixed part of a thread slot. Its table follows at `sizeof(orma_thread_slot)` and its ring
/// at `orma_ring_offset()`. The writer's ring words and the reader's sit on separate lines.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct orma_thread_slot {
    pub state: u32,
    pub tid: u32,
    pub generation: u32,
    pub depth: u32,
    /// The innermost running callback, 0 for none. eBPF reads these 16 bytes at `sched_switch`.
    pub cur_cb: u64,
    pub cur_src: u32,
    pub cur_entry: u32,
    pub table_overflow: u64,
    pub depth_overflow: u64,
    pub reserved0: [u64; 2],
    pub ring_head: u64,
    pub ring_drops: u64,
    pub reserved1: [u64; 6],
    pub ring_tail: u64,
    pub reserved2: [u64; 7],
}

/// A free row has `kind` 0. The writer fills the row and stores `kind` last, with release.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct orma_table_entry {
    pub kind: u16,
    pub src: u16,
    pub entry: u32,
    pub key: u64,
    pub count: u64,
    pub sum: u64,
    pub max: u64,
    pub hist: [u64; ORMA_BUCKETS],
}

/// The C header's trailer repeats these two; the layout test holds them equal.
pub const fn orma_ring_offset(table_entries: u32) -> usize {
    let end =
        size_of::<orma_thread_slot>() + table_entries as usize * size_of::<orma_table_entry>();
    end.next_multiple_of(ORMA_CACHE_LINE)
}

/// `ring_bytes` is a multiple of `ORMA_CACHE_LINE`, so slots start on a line.
pub const fn orma_thread_size(table_entries: u32, ring_bytes: u32) -> usize {
    orma_ring_offset(table_entries) + ring_bytes as usize
}

const _: () = assert!(size_of::<orma_shm_header>() == ORMA_SHM_HEADER_SIZE);
const _: () = assert!(size_of::<orma_thread_slot>() == 3 * ORMA_CACHE_LINE);
const _: () = assert!(size_of::<orma_table_entry>() == 360);
const _: () = assert!(ORMA_SHM_MAGIC == u64::from_le_bytes(*b"ORMASHM\0"));
const _: () = assert!((ORMA_RING_BYTES_DEFAULT as usize).is_multiple_of(ORMA_CACHE_LINE));
