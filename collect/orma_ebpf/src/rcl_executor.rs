//! The uprobe anchors of the rcl_executor core (`rcl_executor/trace.h`), which waits and claims
//! without `rcl_wait` and the rclcpp executor tracepoints (docs/instrumentation.md 2.12). Waits
//! become the stock wait records, keyed by the executor and flagged as the core's.

use aya_ebpf::{macros::uprobe, programs::ProbeContext};
use orma_common::{events::*, shm::ORMA_SRC_ROS};

use crate::{emit::*, maps::*};

// `rcl_executor_item_t` (rcl_executor/executor.h), the same on every LP64 target.
const ITEM_REGISTRATION: u64 = 0;
const ITEM_ENTITY: u64 = 24;
const ITEM_CONTEXT: u64 = 32;

fn a(ctx: &ProbeContext, n: usize) -> u64 {
    ctx.arg::<u64>(n).unwrap_or(0)
}

// void rcl_executor_trace_wait_enter(const rcl_executor_t * executor, int64_t timeout_ns)
#[uprobe]
pub fn rcl_executor_wait_enter(ctx: ProbeContext) -> u32 {
    let ns = now();
    let executor = a(&ctx, 0);
    wait_enter(ORMA_SRC_ROS, executor, ns);
    if streamed(tgid()) {
        let ev = orma_ev_rcl_wait_enter {
            wait_set: executor,
            timeout: a(&ctx, 1) as i64,
        };
        emit_flagged(
            ORMA_EV_RCL_WAIT_ENTER,
            ORMA_RECORD_RCL_EXECUTOR,
            ns,
            &ev,
            [],
        );
    }
    0
}

// void rcl_executor_trace_wait_exit(const rcl_executor_t * executor, rcl_executor_result_t result)
#[uprobe]
pub fn rcl_executor_wait_exit(ctx: ProbeContext) -> u32 {
    let ns = now();
    let executor = a(&ctx, 0);
    wake(ORMA_SRC_ROS, Some(executor), ns);
    if streamed(tgid()) {
        let ev = orma_ev_rcl_wait_exit {
            wait_set: executor,
            ret: ctx.arg::<i32>(1).unwrap_or(0),
            pad: 0,
        };
        emit_flagged(ORMA_EV_RCL_WAIT_EXIT, ORMA_RECORD_RCL_EXECUTOR, ns, &ev, []);
    }
    0
}

// rcl_executor_result_t rcl_executor_next_excluding(rcl_executor_t *, rcl_executor_item_t * item,
//   const rcl_executor_id_t *, size_t): rcl_executor_next jumps here. The claim anchor has the
// registration but not the entity, which the item holds by then.
#[uprobe]
pub fn rcl_executor_next(ctx: ProbeContext) -> u32 {
    if streamed(tgid()) {
        let _ = CLAIMING.insert(tid(), a(&ctx, 1), 0);
    }
    0
}

// void rcl_executor_trace_item_claimed(const rcl_executor_t * executor, rcl_executor_id_t entity,
//   rcl_executor_entity_type_t type, uint64_t group)
#[uprobe]
pub fn rcl_executor_claimed(ctx: ProbeContext) -> u32 {
    let ns = now();
    if !streamed(tgid()) {
        return 0;
    }
    let registration = a(&ctx, 1);
    let stashed = match CLAIMING.get_ptr(tid()) {
        // SAFETY: the entry is this thread's own, read once.
        Some(p) => unsafe { *p },
        None => 0,
    };
    // An item of another call, left when the next probe missed this one, is not read.
    let item =
        (stashed != 0 && read_u64(stashed + ITEM_REGISTRATION) == registration).then_some(stashed);
    let ev = orma_ev_rcl_executor_claim {
        executor: a(&ctx, 0),
        registration,
        entity: item.map_or(0, |i| read_u64(i + ITEM_ENTITY)),
        context: item.map_or(0, |i| read_u64(i + ITEM_CONTEXT)),
        group: a(&ctx, 3),
        entity_type: a(&ctx, 2) as u32,
        rclcpp: 0,
    };
    emit(ORMA_EV_RCL_EXECUTOR_CLAIM, ns, &ev, []);
    0
}

// void rcl_executor_trace_item_completed(const rcl_executor_t * executor, rcl_executor_id_t entity)
#[uprobe]
pub fn rcl_executor_completed(ctx: ProbeContext) -> u32 {
    let ns = now();
    if streamed(tgid()) {
        let ev = orma_ev_rcl_executor_complete {
            executor: a(&ctx, 0),
            registration: a(&ctx, 1),
        };
        emit(ORMA_EV_RCL_EXECUTOR_COMPLETE, ns, &ev, []);
    }
    0
}
