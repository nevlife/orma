//! Callback timing from rclcpp's tracetools functions and flux's `flux_trace_*`. Kernel side
//! only counts: armod reads the maps and does every division and merge.
#![no_std]
#![no_main]
// The BPF target has no compare-and-swap, so core's atomic types lack fetch_add; the intrinsic
// lowers to BPF's atomic add.
#![feature(core_intrinsics)]
#![allow(internal_features)]

use core::intrinsics::{AtomicOrdering, atomic_xadd};

use armo_common::{
    BUCKETS, CallbackKey, CallbackName, CallbackStart, CallbackStat, FluxStart, FluxStartKey,
    KIND_FLUX, KIND_FLUX_CHANNEL, KIND_ROS, LOST_NAME_FULL, LOST_SLOTS, LOST_STAT_FULL, NAME_LEN,
    bucket,
};
use aya_ebpf::{
    bindings::BPF_NOEXIST,
    helpers::{
        bpf_get_current_pid_tgid, bpf_ktime_get_ns, bpf_probe_read_user,
        bpf_probe_read_user_str_bytes,
    },
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

// Per thread: the topic of the message rmw_take last handed over. rclcpp takes and then calls back
// on the same thread, so the callback that starts next is that topic's. Unlike the registration
// events this repeats every message, so it names callbacks armod attached too late to see created.
#[map]
static TAKEN: LruHashMap<u64, CallbackName> = LruHashMap::with_max_entries(MAX_THREADS, 0);

// Apart from STARTS: one thread can run a flux executor and a ROS executor.
#[map]
static FLUX_STARTS: LruHashMap<FluxStartKey, FluxStart> =
    LruHashMap::with_max_entries(MAX_THREADS, 0);

// The channel each flux callback last delivered from, so armod can name it.
#[map]
static FLUX_CHANNELS: HashMap<CallbackKey, u64> = HashMap::with_max_entries(MAX_CALLBACKS, 0);

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

fn key(kind: u32, callback: u64, entry: u32) -> CallbackKey {
    CallbackKey {
        tgid: (bpf_get_current_pid_tgid() >> 32) as u32,
        kind,
        callback,
        entry,
        pad: 0,
    }
}

fn record(k: CallbackKey, ns: u64) {
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
        return;
    };
    // SAFETY: `stat` points to a live map value; each field is updated with an atomic add.
    unsafe {
        add(&raw mut (*stat).count, 1);
        add(&raw mut (*stat).sum_ns, ns);
        add(&raw mut (*stat).hist[bucket(ns)], 1);
    }
}

fn record_name(k: CallbackKey, s: *const u8) {
    let mut name = CallbackName {
        bytes: [0; NAME_LEN],
    };
    // SAFETY: the kernel checks the user address; a bad one returns an error, not a fault.
    if unsafe { bpf_probe_read_user_str_bytes(s, &mut name.bytes) }.is_err() {
        return;
    }
    if NAMES.insert(k, name, 0).is_err() {
        lost(LOST_NAME_FULL);
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
    let tid = bpf_get_current_pid_tgid();
    let _ = STARTS.insert(tid, CallbackStart { callback, ns }, 0);
    if let Some(topic) = TAKEN.get_ptr(tid) {
        // SAFETY: the entry is this thread's own; no other program run writes it.
        let topic = unsafe { *topic };
        let _ = TAKEN.remove(tid);
        // An intra-process callback took nothing; the entry was some earlier take's.
        if ctx.arg::<u8>(1) == Some(0) {
            let _ = NAMES.insert(key(KIND_ROS, callback, 0), topic, BPF_NOEXIST as u64);
        }
    }
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
    record(key(KIND_ROS, callback, 0), now.saturating_sub(start.ns));
    0
}

// ros_trace_rclcpp_callback_register(const void * callback, const char * function_symbol)
#[uprobe]
pub fn callback_register(ctx: ProbeContext) -> u32 {
    let (Some(callback), Some(symbol)) = (ctx.arg::<u64>(0), ctx.arg::<*const u8>(1)) else {
        return 0;
    };
    record_name(key(KIND_ROS, callback, 0), symbol);
    0
}

// ros_trace_rmw_take(const void * rmw_subscription_handle, const void * message,
//                    int64_t source_timestamp, const bool taken)
#[uprobe]
pub fn rmw_take(ctx: ProbeContext) -> u32 {
    let (Some(handle), Some(taken)) = (ctx.arg::<u64>(0), ctx.arg::<u8>(3)) else {
        return 0;
    };
    if taken == 0 || handle == 0 {
        return 0;
    }
    // rmw_subscription_t opens with implementation_identifier, data, topic_name (rmw/types.h).
    let field = (handle + 16) as *const u64;
    // SAFETY: the kernel checks the user address; a bad one returns an error, not a fault.
    let Ok(topic) = (unsafe { bpf_probe_read_user(field) }) else {
        return 0;
    };
    let mut name = CallbackName {
        bytes: [0; NAME_LEN],
    };
    // SAFETY: as above.
    if unsafe { bpf_probe_read_user_str_bytes(topic as *const u8, &mut name.bytes) }.is_err() {
        return 0;
    }
    let _ = TAKEN.insert(bpf_get_current_pid_tgid(), name, 0);
    0
}

// flux_trace_callback_start(const void * executor, uint32_t entry)
#[uprobe]
pub fn flux_callback_start(ctx: ProbeContext) -> u32 {
    let (Some(executor), Some(entry)) = (ctx.arg::<u64>(0), ctx.arg::<u32>(1)) else {
        return 0;
    };
    // SAFETY: a helper call with no pointer arguments.
    let ns = unsafe { bpf_ktime_get_ns() };
    let k = FluxStartKey {
        tid: bpf_get_current_pid_tgid(),
        executor,
    };
    let _ = FLUX_STARTS.insert(k, FluxStart { ns, entry, pad: 0 }, 0);
    0
}

// flux_trace_callback_end(const void * executor, uint32_t entry, int delivered,
//                         const void * channel)
#[uprobe]
pub fn flux_callback_end(ctx: ProbeContext) -> u32 {
    // SAFETY: a helper call with no pointer arguments.
    let now = unsafe { bpf_ktime_get_ns() };
    let (Some(executor), Some(entry), Some(delivered), Some(channel)) = (
        ctx.arg::<u64>(0),
        ctx.arg::<u32>(1),
        ctx.arg::<i32>(2),
        ctx.arg::<u64>(3),
    ) else {
        return 0;
    };
    let sk = FluxStartKey {
        tid: bpf_get_current_pid_tgid(),
        executor,
    };
    let Some(start) = FLUX_STARTS.get_ptr(sk) else {
        return 0;
    };
    // SAFETY: the entry is this thread's own; no other program run writes it.
    let start = unsafe { *start };
    let _ = FLUX_STARTS.remove(sk);
    // A pass that delivered nothing is a probe of the source, not a callback.
    if start.entry != entry || delivered <= 0 {
        return 0;
    }
    let k = key(KIND_FLUX, executor, entry);
    record(k, now.saturating_sub(start.ns));
    if FLUX_CHANNELS.insert(k, channel, 0).is_err() {
        lost(LOST_NAME_FULL);
    }
    0
}

// flux_trace_channel_init(const void * channel, uint64_t dev, uint64_t ino,
//                         const char * segment, int publisher, uint32_t slot_count,
//                         uint32_t slot_size)
#[uprobe]
pub fn flux_channel_init(ctx: ProbeContext) -> u32 {
    let (Some(channel), Some(segment)) = (ctx.arg::<u64>(0), ctx.arg::<*const u8>(3)) else {
        return 0;
    };
    record_name(key(KIND_FLUX_CHANNEL, channel, 0), segment);
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
