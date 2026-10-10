//! flux v1's `flux_trace_*` callback timing, for armod's text mode only. flux v1 has no armo
//! event numbers; these programs stay until flux2 replaces it (design D12) and gain nothing new.

use armo_common::{
    CallbackKey, CallbackName, CallbackStat, FluxStart, FluxStartKey, KIND_FLUX, KIND_FLUX_CHANNEL,
    LOST_NAME_FULL, LOST_STAT_FULL, bucket,
};
use aya_ebpf::{
    bindings::BPF_NOEXIST,
    helpers::{bpf_get_current_pid_tgid, bpf_probe_read_user_str_bytes},
    macros::{map, uprobe},
    maps::{Array, HashMap, LruHashMap},
    programs::ProbeContext,
};

use crate::maps::{add, lost, now};

const MAX_CALLBACKS: u32 = 4096;

// Apart from the ROS stack: one thread can run a flux executor and a ROS executor.
#[map]
static FLUX_STARTS: LruHashMap<FluxStartKey, FluxStart> = LruHashMap::with_max_entries(8192, 0);

// The channel each flux callback last delivered from, so armod can name it.
#[map]
static FLUX_CHANNELS: HashMap<CallbackKey, u64> = HashMap::with_max_entries(MAX_CALLBACKS, 0);

#[map]
static STATS: HashMap<CallbackKey, CallbackStat> = HashMap::with_max_entries(MAX_CALLBACKS, 0);

#[map]
static NAMES: HashMap<CallbackKey, CallbackName> = HashMap::with_max_entries(MAX_CALLBACKS, 0);

#[map]
static ZERO_STAT: Array<CallbackStat> = Array::with_max_entries(1, 0);
#[map]
static ZERO_NAME: Array<CallbackName> = Array::with_max_entries(1, 0);

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
        let Some(zero) = ZERO_STAT.get(0) else {
            return;
        };
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

// flux_trace_callback_start(const void * executor, uint32_t entry)
#[uprobe]
pub fn flux_callback_start(ctx: ProbeContext) -> u32 {
    let (Some(executor), Some(entry)) = (ctx.arg::<u64>(0), ctx.arg::<u32>(1)) else {
        return 0;
    };
    let k = FluxStartKey {
        tid: bpf_get_current_pid_tgid(),
        executor,
    };
    let _ = FLUX_STARTS.insert(
        k,
        FluxStart {
            ns: now(),
            entry,
            pad: 0,
        },
        0,
    );
    0
}

// flux_trace_callback_end(const void * executor, uint32_t entry, int delivered,
//                         const void * channel)
#[uprobe]
pub fn flux_callback_end(ctx: ProbeContext) -> u32 {
    let ns = now();
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
    record(k, ns.saturating_sub(start.ns));
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
    let k = key(KIND_FLUX_CHANNEL, channel, 0);
    let Some(zero) = ZERO_NAME.get(0) else {
        return 0;
    };
    if NAMES.insert(k, zero, 0).is_err() {
        lost(LOST_NAME_FULL);
        return 0;
    }
    let Some(name) = NAMES.get_ptr_mut(k) else {
        return 0;
    };
    // SAFETY: `name` points to a live map value; the kernel checks the user address.
    if unsafe { bpf_probe_read_user_str_bytes(segment, &mut (*name).bytes) }.is_err() {
        let _ = NAMES.remove(k);
    }
    0
}
