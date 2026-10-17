//! Maps and the small helpers every program shares.

use core::intrinsics::{AtomicOrdering, atomic_xadd};

use armo_common::bpf::*;
use aya_ebpf::{
    bindings::BPF_NOEXIST,
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns},
    macros::map,
    maps::{Array, HashMap, LruHashMap, RingBuf},
};

const MAX_THREADS: u32 = 8192;
const MAX_TIDS: u32 = 32768;

/// Records for armod (design 13.4), in the `Out` layout of emit.rs.
#[map]
pub static EVENTS: RingBuf = RingBuf::with_byte_size(8 << 20, 0);

#[map]
pub static CONFIG: Array<u64> = Array::with_max_entries(CONFIG_SLOTS, 0);

/// Processes whose hot records go to the ring buffer; `CONFIG_STREAM_ALL` stands for every one.
#[map]
pub static STREAM: HashMap<u32, u8> = HashMap::with_max_entries(4096, 0);

/// Processes whose threads the scheduler programs follow (design 2.3).
#[map]
pub static SCHED: HashMap<u32, u8> = HashMap::with_max_entries(4096, 0);

/// Processes whose threads' futex waits are recorded (design 20.4).
#[map]
pub static FUTEX: HashMap<u32, u8> = HashMap::with_max_entries(4096, 0);

/// Thread to process, for the scheduler programs: `sched_wakeup` names only the woken thread.
/// armod fills it from /proc, `sched_switch` from the thread leaving the CPU.
#[map]
pub static TID_TGID: LruHashMap<u32, u32> = LruHashMap::with_max_entries(MAX_TIDS, 0);

/// Thread to the address of its shared memory slot's `cur_cb` (design 15.2), from armo-collectd.
#[map]
pub static USER_WORDS: HashMap<u32, u64> = HashMap::with_max_entries(MAX_THREADS, 0);

#[map]
pub static THREADS: LruHashMap<u32, ThreadState> = LruHashMap::with_max_entries(MAX_THREADS, 0);

#[map]
pub static AGG: HashMap<AggKey, AggVal> = HashMap::with_max_entries(16384, 0);

#[map]
pub static OFF: LruHashMap<u32, OffStart> = LruHashMap::with_max_entries(MAX_TIDS, 0);

/// When a followed thread became runnable: woken, or preempted while running.
#[map]
pub static RUNNABLE: LruHashMap<u32, u64> = LruHashMap::with_max_entries(MAX_TIDS, 0);

#[map]
pub static SEEN: LruHashMap<SeenKey, u8> = LruHashMap::with_max_entries(65536, 0);

/// A value a uprobe hands to its uretprobe on the same thread.
#[map]
pub static STASH: LruHashMap<u32, u64> = LruHashMap::with_max_entries(MAX_THREADS, 0);

/// A call an entry program began, for the return program on the same thread: when, an argument
/// and what kind of call.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Pending {
    pub ns: u64,
    pub arg: u64,
    pub kind: u32,
    pub pad: u32,
}

#[map]
pub static CUDA_CALLS: LruHashMap<u32, Pending> = LruHashMap::with_max_entries(MAX_THREADS, 0);

#[map]
pub static DDS_CALLS: LruHashMap<u32, Pending> = LruHashMap::with_max_entries(MAX_THREADS, 0);

#[map]
pub static FUTEX_WAITS: LruHashMap<u32, Pending> = LruHashMap::with_max_entries(MAX_TIDS, 0);

#[map]
pub static LOST: Array<u64> = Array::with_max_entries(LOST_SLOTS_ALL, 0);

#[map]
pub static SAMPLE: Array<u64> = Array::with_max_entries(SAMPLE_SLOTS, 0);

// Never written: the zero values new entries start from. Built on the stack they would pass the
// verifier's 512-byte limit for a program and its calls.
#[map]
pub static ZERO_THREAD: Array<ThreadState> = Array::with_max_entries(1, 0);
#[map]
pub static ZERO_AGG: Array<AggVal> = Array::with_max_entries(1, 0);

pub fn add(counter: *mut u64, v: u64) {
    // SAFETY: `counter` points into a map value the kernel keeps alive for this program run.
    unsafe { atomic_xadd::<u64, u64, { AtomicOrdering::Relaxed }>(counter, v) };
}

pub fn lost(slot: u32) {
    if let Some(p) = LOST.get_ptr_mut(slot) {
        add(p, 1);
    }
}

/// The call this thread began, removed from `map`.
pub fn take(map: &LruHashMap<u32, Pending>) -> Option<Pending> {
    let t = tid();
    // SAFETY: the entry is this thread's own, read once.
    let p = unsafe { *map.get_ptr(t)? };
    let _ = map.remove(t);
    Some(p)
}

pub fn now() -> u64 {
    // SAFETY: a helper call with no pointer arguments.
    unsafe { bpf_ktime_get_ns() }
}

pub fn tgid() -> u32 {
    (bpf_get_current_pid_tgid() >> 32) as u32
}

pub fn tid() -> u32 {
    bpf_get_current_pid_tgid() as u32
}

pub fn config(slot: u32) -> u64 {
    CONFIG.get(slot).copied().unwrap_or(0)
}

/// A return probe attached for one process. The kernel runs every return consumer of a function
/// when any of them took the entry, whatever process each was attached for, so a program
/// attached for pid A also runs on pid B's returns; armod sets the pid as the attach cookie and
/// the program skips other processes (0: attached for all).
pub fn mine(ctx: &impl aya_ebpf::EbpfContext) -> bool {
    // SAFETY: the context pointer is the program's own.
    let cookie = unsafe { aya_ebpf::helpers::generated::bpf_get_attach_cookie(ctx.as_ptr()) };
    cookie == 0 || cookie as u32 == tgid()
}

pub fn streamed(tgid: u32) -> bool {
    config(CONFIG_STREAM_ALL) != 0 || STREAM.get_ptr(tgid).is_some()
}

/// True the first time `(tgid, what, handle)` is seen while the entry lives.
pub fn first(what: u32, handle: u64) -> bool {
    let k = SeenKey {
        tgid: tgid(),
        what,
        handle,
    };
    SEEN.insert(k, 0, BPF_NOEXIST as u64).is_ok()
}

pub fn known(what: u32, handle: u64) -> bool {
    let k = SeenKey {
        tgid: tgid(),
        what,
        handle,
    };
    SEEN.get_ptr(k).is_some()
}

pub fn mark(what: u32, handle: u64) {
    let k = SeenKey {
        tgid: tgid(),
        what,
        handle,
    };
    let _ = SEEN.insert(k, 0, 0);
}
