//! Futex waits of the threads of the processes in `FUTEX` (design 20.4), from the system call
//! tracepoints. armod checks the argument offsets against the kernel's format files (4.2).

use armo_common::events::*;
use aya_ebpf::{macros::tracepoint, programs::TracePointContext};

use crate::{emit::*, maps::*, sched::field};

/// `syscall_trace_enter.args[0]` and `[1]`, and `syscall_trace_exit.ret`.
pub const ENTER_ARG0: usize = 16;
pub const ENTER_ARG1: usize = 24;
pub const EXIT_RET: usize = 16;

const FUTEX_PRIVATE_FLAG: u32 = 128;
const FUTEX_CLOCK_REALTIME: u32 = 256;
const FUTEX_WAIT: u32 = 0;
const FUTEX_LOCK_PI: u32 = 6;
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAIT_REQUEUE_PI: u32 = 11;
const FUTEX_LOCK_PI2: u32 = 13;
/// `op` of a futex_waitv wait, which has no futex() command.
const FUTEX_WAITV: u32 = u32::MAX;
/// `struct futex_waitv { u64 val; u64 uaddr; u32 flags; u32 __reserved; }`
const WAITV_UADDR: u64 = 8;

fn followed() -> bool {
    let tgid = tgid();
    FUTEX.get_ptr(tgid).is_some() && streamed(tgid)
}

/// A wait starts; any other futex call drops what an exit the probes missed left behind.
fn begin(uaddr: u64, op: Option<u32>) -> u32 {
    let Some(op) = op else {
        let _ = FUTEX_WAITS.remove(tid());
        return 0;
    };
    let p = Pending {
        ns: now(),
        arg: uaddr,
        kind: op,
        pad: 0,
    };
    let _ = FUTEX_WAITS.insert(tid(), p, 0);
    0
}

fn end(ctx: &TracePointContext) -> u32 {
    let ns = now();
    let Some(p) = take(&FUTEX_WAITS) else {
        return 0;
    };
    let ev = armo_ev_futex_wait {
        uaddr: p.arg,
        duration_ns: ns.saturating_sub(p.ns),
        op: p.kind,
        ret: field::<i64>(ctx, EXIT_RET).unwrap_or(0) as i32,
    };
    emit(ARMO_EV_FUTEX_WAIT, ns, &ev, []);
    0
}

// futex(u32 *uaddr, int op, ...)
#[tracepoint]
pub fn sys_enter_futex(ctx: TracePointContext) -> u32 {
    if !followed() {
        return 0;
    }
    let (Some(uaddr), Some(op)) = (
        field::<u64>(&ctx, ENTER_ARG0),
        field::<u64>(&ctx, ENTER_ARG1),
    ) else {
        return 0;
    };
    let cmd = op as u32 & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    let wait = matches!(
        cmd,
        FUTEX_WAIT | FUTEX_WAIT_BITSET | FUTEX_LOCK_PI | FUTEX_LOCK_PI2 | FUTEX_WAIT_REQUEUE_PI
    );
    begin(uaddr, wait.then_some(cmd))
}

#[tracepoint]
pub fn sys_exit_futex(ctx: TracePointContext) -> u32 {
    end(&ctx)
}

// futex_waitv(struct futex_waitv *waiters, ...): the first waiter's address stands for the wait.
#[tracepoint]
pub fn sys_enter_futex_waitv(ctx: TracePointContext) -> u32 {
    if !followed() {
        return 0;
    }
    let Some(waiters) = field::<u64>(&ctx, ENTER_ARG0) else {
        return 0;
    };
    begin(read_u64(waiters + WAITV_UADDR), Some(FUTEX_WAITV))
}

#[tracepoint]
pub fn sys_exit_futex_waitv(ctx: TracePointContext) -> u32 {
    end(&ctx)
}
