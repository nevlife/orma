//! Callback timing from rclcpp's tracetools functions. Kernel side only counts: armod reads the
//! maps and does every division and merge.
#![no_std]
#![no_main]
// The BPF target has no compare-and-swap, so core's atomic types lack fetch_add; the intrinsic
// lowers to BPF's atomic add.
#![feature(core_intrinsics)]
#![allow(internal_features)]

use core::intrinsics::{AtomicOrdering, atomic_xadd};

use armo_common::{
    BUCKETS, CallbackKey, CallbackName, CallbackStart, CallbackStat, LOST_NAME_FULL, LOST_SLOTS,
    LOST_STAT_FULL, NAME_LEN, bucket,
};
use aya_ebpf::{
    bindings::BPF_NOEXIST,
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns, bpf_probe_read_user_str_bytes},
    macros::{map, uprobe},
    maps::{Array, HashMap, LruHashMap},
    programs::ProbeContext,
};

const MAX_CALLBACKS: u32 = 4096;
const MAX_THREADS: u32 = 8192;

// Keyed by thread: only that thread writes its entry. LRU so a thread that dies inside a callback
// does not hold a slot forever.
#[map]
static STARTS: LruHashMap<u64, CallbackStart> = LruHashMap::with_max_entries(MAX_THREADS, 0);

#[map]
static STATS: HashMap<CallbackKey, CallbackStat> = HashMap::with_max_entries(MAX_CALLBACKS, 0);

#[map]
static NAMES: HashMap<CallbackKey, CallbackName> = HashMap::with_max_entries(MAX_CALLBACKS, 0);

#[map]
static LOST: Array<u64> = Array::with_max_entries(LOST_SLOTS, 0);

fn add(counter: *mut u64, v: u64) {
    // SAFETY: `counter` points into a map value the kernel keeps alive for this program run.
    unsafe { atomic_xadd::<u64, u64, { AtomicOrdering::Relaxed }>(counter, v) };
}

fn lost(slot: u32) {
    if let Some(p) = LOST.get_ptr_mut(slot) {
        add(p, 1);
    }
}

fn key(callback: u64) -> CallbackKey {
    CallbackKey {
        tgid: (bpf_get_current_pid_tgid() >> 32) as u32,
        pad: 0,
        callback,
    }
}

// ros_trace_callback_start(const void * callback, bool is_intra_process)
#[uprobe]
pub fn callback_start(ctx: ProbeContext) -> u32 {
    let Some(callback) = ctx.arg::<u64>(0) else {
        return 0;
    };
    // SAFETY: a helper call with no pointer arguments.
    let ns = unsafe { bpf_ktime_get_ns() };
    let _ = STARTS.insert(
        bpf_get_current_pid_tgid(),
        CallbackStart { callback, ns },
        0,
    );
    0
}

// ros_trace_callback_end(const void * callback)
#[uprobe]
pub fn callback_end(ctx: ProbeContext) -> u32 {
    // SAFETY: a helper call with no pointer arguments.
    let now = unsafe { bpf_ktime_get_ns() };
    let Some(callback) = ctx.arg::<u64>(0) else {
        return 0;
    };
    let tid = bpf_get_current_pid_tgid();
    let Some(start) = STARTS.get_ptr(tid) else {
        return 0;
    };
    // SAFETY: the entry is this thread's own; no other program run writes it.
    let start = unsafe { *start };
    let _ = STARTS.remove(tid);
    if start.callback != callback {
        return 0;
    }
    let ns = now.saturating_sub(start.ns);
    let k = key(callback);
    if STATS.get_ptr_mut(k).is_none() {
        let zero = CallbackStat {
            count: 0,
            sum_ns: 0,
            hist: [0; BUCKETS],
        };
        // Another CPU may create it first; BPF_NOEXIST keeps whichever came first.
        let _ = STATS.insert(k, zero, BPF_NOEXIST as u64);
    }
    let Some(stat) = STATS.get_ptr_mut(k) else {
        lost(LOST_STAT_FULL);
        return 0;
    };
    // SAFETY: `stat` points to a live map value; each field is updated with an atomic add.
    unsafe {
        add(&raw mut (*stat).count, 1);
        add(&raw mut (*stat).sum_ns, ns);
        add(&raw mut (*stat).hist[bucket(ns)], 1);
    }
    0
}

// ros_trace_rclcpp_callback_register(const void * callback, const char * function_symbol)
#[uprobe]
pub fn callback_register(ctx: ProbeContext) -> u32 {
    let (Some(callback), Some(symbol)) = (ctx.arg::<u64>(0), ctx.arg::<*const u8>(1)) else {
        return 0;
    };
    let mut name = CallbackName {
        bytes: [0; NAME_LEN],
    };
    // SAFETY: the kernel checks the user address; a bad one returns an error, not a fault.
    if unsafe { bpf_probe_read_user_str_bytes(symbol, &mut name.bytes) }.is_err() {
        return 0;
    }
    if NAMES.insert(key(callback), name, 0).is_err() {
        lost(LOST_NAME_FULL);
    }
    0
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
