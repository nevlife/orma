//! `flux_trace_*` of flux_trace_contract_2 in libflux_ffi.so (design 11.2). armod attaches
//! only to a library that exports the contract symbol.

use armo_common::{events::*, shm::ARMO_SRC_FLUX};
use aya_ebpf::{macros::uprobe, programs::ProbeContext};

use crate::{emit::*, maps::*};

fn a(ctx: &ProbeContext, n: usize) -> u64 {
    // x86-64 passes six arguments in registers. At the entry uprobe [sp] is the return address
    // and the 7th and later arguments follow in 8-byte slots.
    #[cfg(bpf_target_arch = "x86_64")]
    if n >= 6 {
        // SAFETY: regs is the probe's pt_regs; the kernel checks the user stack address.
        let sp = unsafe { (*ctx.regs).rsp };
        let slot = (sp + 8 * (n as u64 - 5)) as *const u64;
        return unsafe { aya_ebpf::helpers::bpf_probe_read_user(slot) }.unwrap_or(0);
    }
    ctx.arg::<u64>(n).unwrap_or(0)
}

/// A 32-bit argument: the register's upper half is not defined.
fn w(ctx: &ProbeContext, n: usize) -> u32 {
    a(ctx, n) as u32
}

fn hot<T: Copy + 'static>(event: u16, p: &T) {
    let ns = now();
    if streamed(tgid()) {
        emit(event, ns, p, []);
    }
}

// channel_init(channel, dev, ino, segment, publisher, slot_count, slot_size, stamp_offset)
#[uprobe]
pub fn flux_channel_init(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_channel_init {
        channel: a(&ctx, 0),
        dev: a(&ctx, 1),
        ino: a(&ctx, 2),
        publisher: w(&ctx, 4) as i32,
        slot_count: w(&ctx, 5),
        slot_size: w(&ctx, 6),
        stamp_offset: w(&ctx, 7) as i32,
    };
    emit(ARMO_EV_FLUX_CHANNEL_INIT, now(), &ev, [a(&ctx, 3)]);
    0
}

// endpoint_init(endpoint, node, key, signpost, fingerprint, role, lang)
#[uprobe]
pub fn flux_endpoint_init(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_endpoint_init {
        endpoint: a(&ctx, 0),
        fingerprint: a(&ctx, 4),
        role: w(&ctx, 5) as i32,
        lang: w(&ctx, 6) as i32,
    };
    let strings = [a(&ctx, 1), a(&ctx, 2), a(&ctx, 3)];
    emit(ARMO_EV_FLUX_ENDPOINT_INIT, now(), &ev, strings);
    0
}

// claim(channel, slot, result, loan)
#[uprobe]
pub fn flux_claim(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_claim {
        channel: a(&ctx, 0),
        slot: w(&ctx, 1),
        result: w(&ctx, 2) as i32,
        loan: w(&ctx, 3) as i32,
        pad: 0,
    };
    hot(ARMO_EV_FLUX_CLAIM, &ev);
    0
}

// loan(channel, slot)
#[uprobe]
pub fn flux_loan(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_loan {
        channel: a(&ctx, 0),
        slot: w(&ctx, 1),
        pad: 0,
    };
    hot(ARMO_EV_FLUX_LOAN, &ev);
    0
}

// loan_end(channel, slot, result)
#[uprobe]
pub fn flux_loan_end(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_loan_end {
        channel: a(&ctx, 0),
        slot: w(&ctx, 1),
        result: w(&ctx, 2) as i32,
    };
    hot(ARMO_EV_FLUX_LOAN_END, &ev);
    0
}

// commit(channel, ticket, slot, nbytes, woke, stamp)
#[uprobe]
pub fn flux_commit(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_commit {
        channel: a(&ctx, 0),
        ticket: a(&ctx, 1),
        nbytes: a(&ctx, 3),
        stamp: a(&ctx, 5),
        slot: w(&ctx, 2),
        woke: w(&ctx, 4) as i32,
    };
    hot(ARMO_EV_FLUX_COMMIT, &ev);
    0
}

// take(channel, ticket, slot, lost, stamp)
#[uprobe]
pub fn flux_take(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_take {
        channel: a(&ctx, 0),
        ticket: a(&ctx, 1),
        lost: a(&ctx, 3),
        stamp: a(&ctx, 4),
        slot: w(&ctx, 2),
        pad: 0,
    };
    hot(ARMO_EV_FLUX_TAKE, &ev);
    0
}

// release(channel, ticket, held)
#[uprobe]
pub fn flux_release(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_release {
        channel: a(&ctx, 0),
        ticket: a(&ctx, 1),
        held: w(&ctx, 2) as i32,
        pad: 0,
    };
    hot(ARMO_EV_FLUX_RELEASE, &ev);
    0
}

// refused(channel, reason)
#[uprobe]
pub fn flux_refused(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_refused {
        channel: a(&ctx, 0),
        reason: w(&ctx, 1) as i32,
        pad: 0,
    };
    hot(ARMO_EV_FLUX_REFUSED, &ev);
    0
}

// wait(executor, sources, timeout_ns)
#[uprobe]
pub fn flux_wait(ctx: ProbeContext) -> u32 {
    let ns = now();
    let executor = a(&ctx, 0);
    wait_enter(ARMO_SRC_FLUX, executor, ns);
    if streamed(tgid()) {
        let ev = armo_ev_flux_wait {
            executor,
            timeout_ns: a(&ctx, 2) as i64,
            sources: w(&ctx, 1),
            pad: 0,
        };
        emit(ARMO_EV_FLUX_WAIT, ns, &ev, []);
    }
    0
}

// wake(executor, result, entry)
#[uprobe]
pub fn flux_wake(ctx: ProbeContext) -> u32 {
    let ns = now();
    let executor = a(&ctx, 0);
    wake(ARMO_SRC_FLUX, Some(executor), ns);
    if streamed(tgid()) {
        let ev = armo_ev_flux_wake {
            executor,
            result: w(&ctx, 1) as i32,
            entry: w(&ctx, 2),
        };
        emit(ARMO_EV_FLUX_WAKE, ns, &ev, []);
    }
    0
}

// callback_start(executor, entry, endpoint)
#[uprobe]
pub fn flux_callback_start(ctx: ProbeContext) -> u32 {
    let ns = now();
    let executor = a(&ctx, 0);
    let entry = w(&ctx, 1);
    callback_start(ARMO_SRC_FLUX, executor, entry, ns);
    if streamed(tgid()) {
        let ev = armo_ev_flux_callback_start {
            executor,
            endpoint: a(&ctx, 2),
            entry,
            pad: 0,
        };
        emit(ARMO_EV_FLUX_CALLBACK_START, ns, &ev, []);
    }
    0
}

// callback_end(executor, entry, delivered, channel, segment). A pass that delivered nothing is
// a probe of the source, not a callback.
#[uprobe]
pub fn flux_callback_end(ctx: ProbeContext) -> u32 {
    let ns = now();
    let executor = a(&ctx, 0);
    let entry = w(&ctx, 1);
    let delivered = w(&ctx, 2) as i32;
    let start = callback_end(ARMO_SRC_FLUX, executor, entry);
    if start != 0 && delivered > 0 {
        agg(
            armo_common::shm::ARMO_AGG_CALLBACK,
            ARMO_SRC_FLUX,
            entry,
            executor,
            tgid(),
            tid(),
            ns.saturating_sub(start),
        );
    }
    if streamed(tgid()) {
        let ev = armo_ev_flux_callback_end {
            executor,
            channel: a(&ctx, 3),
            entry,
            delivered,
        };
        emit(ARMO_EV_FLUX_CALLBACK_END, ns, &ev, [a(&ctx, 4)]);
    }
    0
}

// py_gil(phase)
#[uprobe]
pub fn flux_py_gil(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_py_gil {
        phase: w(&ctx, 0) as i32,
        pad: 0,
    };
    hot(ARMO_EV_FLUX_PY_GIL, &ev);
    0
}

// py_ready(channel, ticket)
#[uprobe]
pub fn flux_py_ready(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_py_ready {
        channel: a(&ctx, 0),
        ticket: a(&ctx, 1),
    };
    hot(ARMO_EV_FLUX_PY_READY, &ev);
    0
}

// record_drop(channel, ticket, count, reason)
#[uprobe]
pub fn flux_record_drop(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_record_drop {
        channel: a(&ctx, 0),
        ticket: a(&ctx, 1),
        count: w(&ctx, 2),
        reason: w(&ctx, 3) as i32,
    };
    hot(ARMO_EV_FLUX_RECORD_DROP, &ev);
    0
}

// gpu_wedged(result): once per process, so it rides with the discovery probes.
#[uprobe]
pub fn flux_gpu_wedged(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_flux_gpu_wedged {
        result: w(&ctx, 0) as i32,
        pad: 0,
    };
    emit(ARMO_EV_FLUX_GPU_WEDGED, now(), &ev, []);
    0
}
