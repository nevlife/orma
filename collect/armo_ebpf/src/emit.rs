//! Records into the ring buffer, and the per-thread state behind the in-process intervals.

use core::ffi::c_void;

use armo_common::{
    bpf::*,
    bucket,
    events::{armo_position, armo_record},
};
use aya_ebpf::{
    bindings::{BPF_NOEXIST, BPF_RB_NO_WAKEUP},
    helpers::{bpf_probe_read_user, generated::bpf_probe_read_user_str},
};

use crate::maps::*;

#[repr(C)]
pub struct Out<T, const N: usize> {
    pub pos: armo_position,
    pub rec: armo_record,
    pub payload: T,
    pub strings: [[u8; STRING_SLOT]; N],
}

/// One record of this thread. Each string is read into its own slot; armod packs them.
#[inline(always)]
pub fn emit<T: Copy + 'static, const N: usize>(
    event: u16,
    ns: u64,
    payload: &T,
    strings: [u64; N],
) {
    let Some(mut entry) = EVENTS.reserve::<Out<T, N>>(0) else {
        lost(LOST_RING);
        return;
    };
    let out = entry.as_mut_ptr();
    // SAFETY: `out` is the reserved entry, sized for `Out<T, N>`; the string reads check the
    // user addresses and fail rather than fault.
    unsafe {
        (*out).pos = armo_position {
            pid: tgid(),
            tid: tid(),
        };
        (*out).rec = armo_record {
            event,
            len: 0,
            flags: 0,
            t_mono_ns: ns,
        };
        (*out).payload = *payload;
        let mut i = 0;
        while i < N {
            let slot = (*out).strings[i].as_mut_ptr();
            if bpf_probe_read_user_str(
                slot.cast::<c_void>(),
                STRING_SLOT as u32,
                strings[i] as *const c_void,
            ) < 0
            {
                *slot = 0;
            }
            i += 1;
        }
    }
    // armod reads the ring on its own tick and never waits on it, so the wakeup (an irq_work
    // on this CPU, a kthread on PREEMPT_RT) would buy nothing.
    entry.submit(BPF_RB_NO_WAKEUP as u64);
}

pub fn read_u64(addr: u64) -> u64 {
    // SAFETY: the kernel checks the user address; a bad one returns an error, not a fault.
    unsafe { bpf_probe_read_user(addr as *const u64) }.unwrap_or(0)
}

/// The 8 bytes at a message: `header.stamp` when the message opens with a Header (design 2.1.2).
pub fn stamp(message: u64) -> u64 {
    read_u64(message)
}

/// Adds one sample to the aggregate row (design 13.2). Each row has one writer at a time: the
/// thread its `tid` names, or the CPU switching that thread in.
#[inline(always)]
pub fn agg(kind: u16, src: u16, entry: u32, key: u64, tgid: u32, tid: u32, v: u64) {
    let k = AggKey {
        tgid,
        tid,
        kind,
        src,
        entry,
        key,
    };
    if AGG.get_ptr_mut(k).is_none() {
        let Some(zero) = ZERO_AGG.get(0) else {
            return;
        };
        let _ = AGG.insert(k, zero, BPF_NOEXIST as u64);
    }
    let Some(row) = AGG.get_ptr_mut(k) else {
        lost(LOST_AGG_FULL);
        return;
    };
    let generation = config(CONFIG_GEN);
    // SAFETY: `row` points to a live map value.
    unsafe {
        add(&raw mut (*row).count, 1);
        add(&raw mut (*row).sum, v);
        add(&raw mut (*row).hist[bucket(v)], 1);
        if (*row).max_gen != generation {
            (*row).max = v;
            (*row).max_gen = generation;
        } else if v > (*row).max {
            (*row).max = v;
        }
    }
}

/// This thread's state, created on first use.
#[inline(always)]
pub fn thread() -> Option<*mut ThreadState> {
    let t = tid();
    if let Some(p) = THREADS.get_ptr_mut(t) {
        return Some(p);
    }
    let zero = ZERO_THREAD.get(0)?;
    let _ = THREADS.insert(t, zero, BPF_NOEXIST as u64);
    let p = THREADS.get_ptr_mut(t);
    if p.is_none() {
        lost(LOST_THREAD_FULL);
    }
    p
}

/// A callback starts: push it, and time the dispatch from this thread's last wake at the same
/// depth (design 2.5). A flux callback's wake must also be its executor's.
#[inline(always)]
pub fn callback_start(src: u16, key: u64, entry: u32, ns: u64) {
    let Some(t) = thread() else {
        return;
    };
    // SAFETY: the entry is this thread's own; only its own uprobes write it.
    unsafe {
        let d = (*t).depth;
        if (*t).wake_ns != 0
            && (*t).wake_depth == d
            && (*t).wake_src == src as u32
            && (src == armo_common::shm::ARMO_SRC_ROS || (*t).wake_key == key)
        {
            let wait = ns.saturating_sub((*t).wake_ns);
            agg(
                armo_common::shm::ARMO_AGG_DISPATCH,
                src,
                entry,
                key,
                tgid(),
                tid(),
                wait,
            );
        }
        (*t).depth = d + 1;
        if d as usize >= STACK_DEPTH {
            lost(LOST_DEPTH);
            return;
        }
        (*t).frames[d as usize & (STACK_DEPTH - 1)] = Frame {
            key,
            ns,
            src: src as u32,
            entry,
        };
    }
}

/// A callback ends: the innermost frame of the same key, searched from the top because rclpy
/// coroutine callbacks can end out of order. Returns its start, or 0 for an end with no start.
/// A start past the stack's depth is consumed by the next end without a time.
#[inline(always)]
pub fn callback_end(src: u16, key: u64, entry: u32) -> u64 {
    let Some(t) = THREADS.get_ptr_mut(tid()) else {
        lost(LOST_UNPAIRED);
        return 0;
    };
    // SAFETY: as in `callback_start`.
    unsafe {
        let d = (*t).depth;
        if d == 0 {
            return 0;
        }
        if d as usize > STACK_DEPTH {
            (*t).depth = d - 1;
            return 0;
        }
        // Indexes are masked so the verifier sees them bounded in every unrolled step.
        let mask = STACK_DEPTH - 1;
        let mut start = 0;
        let mut at = STACK_DEPTH;
        let mut j = 0;
        while j < STACK_DEPTH && (j as u32) < d {
            let i = (d as usize - 1 - j) & mask;
            let f = &(*t).frames[i];
            if f.src == src as u32 && f.key == key && f.entry == entry {
                start = f.ns;
                at = i;
                break;
            }
            j += 1;
        }
        if at == STACK_DEPTH {
            return 0;
        }
        (*t).frames[at & mask].src = 0;
        (*t).frames[at & mask].key = 0;
        // Pop the top and any frames ended out of order beneath it.
        let mut top = d;
        let mut j = 0;
        while j < STACK_DEPTH && top > 0 {
            if (*t).frames[(top as usize - 1) & mask].src != 0 {
                break;
            }
            top -= 1;
            j += 1;
        }
        (*t).depth = top;
        start
    }
}

/// The innermost running callback of a thread's state, for `sched_switch`.
#[inline(always)]
pub fn top(t: *const ThreadState) -> Option<Frame> {
    // SAFETY: a map value; a concurrent write by the thread itself can only tear the frame, which
    // shifts one boundary.
    unsafe {
        let d = (*t).depth as usize;
        if d == 0 || d > STACK_DEPTH {
            return None;
        }
        let f = (*t).frames[(d - 1) & (STACK_DEPTH - 1)];
        (f.src != 0).then_some(f)
    }
}

pub fn wait_enter(src: u16, key: u64, ns: u64) {
    let Some(t) = thread() else {
        return;
    };
    // SAFETY: as in `callback_start`.
    unsafe {
        (*t).wait_ns = ns;
        (*t).wait_key = key;
        (*t).wait_src = src as u32;
        (*t).wake_ns = 0;
    }
}

/// A wake ends the wait it matches and starts the dispatch interval. Returns the waited key.
#[inline(always)]
pub fn wake(src: u16, key: Option<u64>, ns: u64) -> u64 {
    let Some(t) = thread() else {
        return 0;
    };
    // SAFETY: as in `callback_start`.
    unsafe {
        let waited = (*t).wait_src == src as u32 && key.is_none_or(|k| k == (*t).wait_key);
        let k = key.unwrap_or((*t).wait_key);
        if waited {
            let w = ns.saturating_sub((*t).wait_ns);
            agg(armo_common::shm::ARMO_AGG_WAIT, src, 0, k, tgid(), tid(), w);
            (*t).wait_src = 0;
        }
        (*t).wake_ns = ns;
        (*t).wake_key = k;
        (*t).wake_src = src as u32;
        (*t).wake_depth = (*t).depth;
        k
    }
}
