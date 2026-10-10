//! `sched_switch` and `sched_wakeup` (design 2.3, 15.2): CPU wait and off-CPU time inside a
//! callback, for the threads of the processes in `SCHED`. Only the tracepoint arguments are read;
//! armod checks their offsets against the kernel's format files before it attaches (4.2).

use armo_common::{
    bpf::*,
    events::*,
    shm::{ARMO_AGG_CPU_WAIT, ARMO_AGG_OFFCPU},
};
use aya_ebpf::{
    helpers::{bpf_get_smp_processor_id, bpf_probe_read_user},
    macros::tracepoint,
    programs::TracePointContext,
};

use crate::{emit::*, maps::*};

pub const SWITCH_PREV_PID: usize = 24;
pub const SWITCH_PREV_STATE: usize = 32;
pub const SWITCH_NEXT_PID: usize = 56;
pub const WAKEUP_PID: usize = 24;
pub const WAKEUP_TARGET_CPU: usize = 32;

/// `prev_state` bits below this are sleep states; none set means it was still runnable
/// (TASK_REPORT_MAX of this kernel's format file).
const STATE_MASK: i64 = 0xff;

fn field<T>(ctx: &TracePointContext, offset: usize) -> Option<T> {
    // SAFETY: a read of the tracepoint's own record; a bad offset returns an error.
    unsafe { ctx.read_at::<T>(offset) }.ok()
}

fn followed(tgid: u32) -> bool {
    tgid != 0 && SCHED.get_ptr(tgid).is_some()
}

/// The callback a thread leaving the CPU is in: the shared memory word the LD_PRELOAD or LTTng
/// shim keeps (its 16 bytes: `cur_cb`, `cur_src`, `cur_entry`), else the uprobe stack.
fn current_callback(tid: u32) -> Option<(u64, u32, u32)> {
    if let Some(addr) = USER_WORDS.get_ptr(tid) {
        // SAFETY: the entry is a plain u64; the user read is checked by the kernel.
        let addr = unsafe { *addr };
        let Ok(words) = (unsafe { bpf_probe_read_user(addr as *const [u64; 2]) }) else {
            lost(LOST_USER_WORD);
            return None;
        };
        return (words[0] != 0).then_some((words[0], words[1] as u32, (words[1] >> 32) as u32));
    }
    let t = THREADS.get_ptr(tid)?;
    let f = top(t)?;
    Some((f.key, f.src, f.entry))
}

#[tracepoint]
pub fn sched_switch(ctx: TracePointContext) -> u32 {
    let ns = now();
    let (Some(prev), Some(state), Some(next)) = (
        field::<i32>(&ctx, SWITCH_PREV_PID),
        field::<i64>(&ctx, SWITCH_PREV_STATE),
        field::<i32>(&ctx, SWITCH_NEXT_PID),
    ) else {
        return 0;
    };
    let (prev, next) = (prev as u32, next as u32);

    let mut next_tgid = 0;
    if let Some(o) = OFF.get_ptr(next) {
        // SAFETY: a map value read once.
        let o = unsafe { *o };
        let _ = OFF.remove(next);
        next_tgid = o.tgid;
        agg(
            ARMO_AGG_OFFCPU,
            o.src as u16,
            o.entry,
            o.key,
            o.tgid,
            next,
            ns.saturating_sub(o.ns),
        );
    }
    if let Some(r) = RUNNABLE.get_ptr(next) {
        // SAFETY: as above.
        let since = unsafe { *r };
        let _ = RUNNABLE.remove(next);
        if next_tgid == 0 {
            // SAFETY: as above.
            next_tgid = TID_TGID.get_ptr(next).map_or(0, |g| unsafe { *g });
        }
        if next_tgid != 0 {
            agg(
                ARMO_AGG_CPU_WAIT,
                0,
                0,
                next as u64,
                next_tgid,
                next,
                ns.saturating_sub(since),
            );
        }
    }

    let prev_tgid = tgid();
    let prev_followed = prev != 0 && followed(prev_tgid);
    if prev_followed {
        if TID_TGID.get_ptr(prev).is_none() {
            let _ = TID_TGID.insert(prev, prev_tgid, 0);
        }
        if let Some((key, src, entry)) = current_callback(prev) {
            let o = OffStart {
                ns,
                key,
                src,
                entry,
                tgid: prev_tgid,
                pad: 0,
            };
            let _ = OFF.insert(prev, o, 0);
        }
        if state & STATE_MASK == 0 {
            let _ = RUNNABLE.insert(prev, ns, 0);
        }
    }

    if next_tgid == 0 && next != 0 {
        // SAFETY: as above.
        next_tgid = TID_TGID.get_ptr(next).map_or(0, |g| unsafe { *g });
    }
    let record =
        (prev_followed && streamed(prev_tgid)) || (followed(next_tgid) && streamed(next_tgid));
    if record {
        let ev = armo_ev_sched_switch {
            prev_state: state,
            next_tid: next,
            // SAFETY: a helper call with no arguments.
            cpu: unsafe { bpf_get_smp_processor_id() },
        };
        emit(ARMO_EV_SCHED_SWITCH, ns, &ev, []);
    }
    0
}

#[tracepoint]
pub fn sched_wakeup(ctx: TracePointContext) -> u32 {
    let ns = now();
    let (Some(woken), Some(cpu)) = (
        field::<i32>(&ctx, WAKEUP_PID),
        field::<i32>(&ctx, WAKEUP_TARGET_CPU),
    ) else {
        return 0;
    };
    let woken = woken as u32;
    let Some(g) = TID_TGID.get_ptr(woken) else {
        return 0;
    };
    // SAFETY: a map value read once.
    let woken_tgid = unsafe { *g };
    if !followed(woken_tgid) {
        return 0;
    }
    let _ = RUNNABLE.insert(woken, ns, 0);
    if streamed(woken_tgid) {
        let ev = armo_ev_sched_wakeup {
            tid: woken,
            target_cpu: cpu as u32,
        };
        emit(ARMO_EV_SCHED_WAKEUP, ns, &ev, []);
    }
    0
}
