//! Delivery paths of the ROS 2 overhaul (docs/instrumentation.md 2.13): the rcl broker's anchors,
//! the rcl loan functions, zenoh-c's shared memory payload and rclpy's conversion anchors. All
//! entry probes but the borrow, whose loan only its return knows.

use aya_ebpf::{
    macros::{uprobe, uretprobe},
    programs::{ProbeContext, RetProbeContext},
};
use orma_common::{bpf::*, events::*};

use crate::{emit::*, maps::*};

fn a(ctx: &ProbeContext, n: usize) -> u64 {
    ctx.arg::<u64>(n).unwrap_or(0)
}

fn hot<T: Copy + 'static>(event: u16, ns: u64, p: &T) {
    if streamed(tgid()) {
        emit(event, ns, p, []);
    }
}

// void rcl_ipc_trace_publish(const rcl_publisher_t *, const void * message,
//   int64_t source_timestamp, size_t readers)
#[uprobe]
pub fn rcl_ipc_publish(ctx: ProbeContext) -> u32 {
    let ev = orma_ev_rcl_ipc_publish {
        publisher: a(&ctx, 0),
        message: a(&ctx, 1),
        source_timestamp: a(&ctx, 2) as i64,
        readers: a(&ctx, 3),
    };
    hot(ORMA_EV_RCL_IPC_PUBLISH, now(), &ev);
    0
}

// void rcl_ipc_trace_take(const rcl_subscription_t *, const rcl_ipc_message_t * message,
//   const void * data, int64_t source_timestamp, bool local)
#[uprobe]
pub fn rcl_ipc_take(ctx: ProbeContext) -> u32 {
    let ns = now();
    if !streamed(tgid()) {
        return 0;
    }
    let data = a(&ctx, 2);
    let ev = orma_ev_rcl_ipc_take {
        subscription: a(&ctx, 0),
        message: a(&ctx, 1),
        data,
        source_timestamp: a(&ctx, 3) as i64,
        stamp: if data != 0 { stamp(data) } else { 0 },
        local: (a(&ctx, 4) & 0xff != 0) as u32,
        pad: 0,
    };
    emit(ORMA_EV_RCL_IPC_TAKE, ns, &ev, []);
    0
}

// void rcl_ipc_trace_release(const rcl_ipc_message_t * message)
#[uprobe]
pub fn rcl_ipc_release(ctx: ProbeContext) -> u32 {
    hot(
        ORMA_EV_RCL_IPC_RELEASE,
        now(),
        &orma_ev_handle { handle: a(&ctx, 0) },
    );
    0
}

// rcl_ret_t rcl_borrow_loaned_message(const rcl_publisher_t *, const rosidl_message_type_support_t *,
//   void ** ros_message)
#[uprobe]
pub fn rcl_loan_borrow(ctx: ProbeContext) -> u32 {
    if streamed(tgid()) {
        let call = Loan {
            endpoint: a(&ctx, 0),
            out: a(&ctx, 2),
        };
        let _ = LOAN_CALLS.insert(tid(), call, 0);
    }
    0
}

#[uretprobe]
pub fn rcl_loan_borrow_ret(ctx: RetProbeContext) -> u32 {
    let ns = now();
    if !mine(&ctx) || !streamed(tgid()) {
        return 0;
    }
    let t = tid();
    let Some(call) = LOAN_CALLS.get_ptr(t) else {
        lost(LOST_UNPAIRED);
        return 0;
    };
    // SAFETY: the entry is this thread's own, read once.
    let call = unsafe { *call };
    let _ = LOAN_CALLS.remove(t);
    let ret = ctx.ret::<i32>();
    let ev = orma_ev_rcl_loan {
        endpoint: call.endpoint,
        message: if ret == 0 && call.out != 0 {
            read_u64(call.out)
        } else {
            0
        },
        ret,
        subscription: 0,
    };
    emit(ORMA_EV_RCL_LOAN_BORROW, ns, &ev, []);
    0
}

fn loan(ctx: &ProbeContext, event: u16, message: u64, subscription: u32) {
    let ev = orma_ev_rcl_loan {
        endpoint: a(ctx, 0),
        message,
        ret: 0,
        subscription,
    };
    hot(event, now(), &ev);
}

// rcl_ret_t rcl_publish_loaned_message(const rcl_publisher_t *, void * ros_message, allocation)
#[uprobe]
pub fn rcl_loan_publish(ctx: ProbeContext) -> u32 {
    loan(&ctx, ORMA_EV_RCL_LOAN_PUBLISH, a(&ctx, 1), 0);
    0
}

// rcl_ret_t rcl_return_loaned_message_from_publisher(const rcl_publisher_t *, void * loaned)
#[uprobe]
pub fn rcl_loan_return_pub(ctx: ProbeContext) -> u32 {
    loan(&ctx, ORMA_EV_RCL_LOAN_RETURN, a(&ctx, 1), 0);
    0
}

// rcl_ret_t rcl_take_loaned_message(const rcl_subscription_t *, void ** loaned, info, allocation)
#[uprobe]
pub fn rcl_loan_take(ctx: ProbeContext) -> u32 {
    loan(&ctx, ORMA_EV_RCL_LOAN_TAKE, 0, 1);
    0
}

// rcl_ret_t rcl_return_loaned_message_from_subscription(const rcl_subscription_t *, void * loaned)
#[uprobe]
pub fn rcl_loan_return_sub(ctx: ProbeContext) -> u32 {
    loan(&ctx, ORMA_EV_RCL_LOAN_RETURN, a(&ctx, 1), 1);
    0
}

// z_result_t z_bytes_from_shm_mut(z_owned_bytes_t * this_, z_moved_shm_mut_t * shm)
#[uprobe]
pub fn zenoh_shm_payload(ctx: ProbeContext) -> u32 {
    hot(
        ORMA_EV_ZENOH_SHM_PAYLOAD,
        now(),
        &orma_ev_handle { handle: a(&ctx, 0) },
    );
    0
}

// void rclpy_trace_take_converted(const rcl_subscription_t *, const PyObject * message)
#[uprobe]
pub fn rclpy_take_converted(ctx: ProbeContext) -> u32 {
    let ev = orma_ev_rclpy_take {
        subscription: a(&ctx, 0),
        message: a(&ctx, 1),
    };
    hot(ORMA_EV_RCLPY_TAKE_CONVERTED, now(), &ev);
    0
}

// void rclpy_trace_publish_enter(const rcl_publisher_t *)
#[uprobe]
pub fn rclpy_publish_enter(ctx: ProbeContext) -> u32 {
    hot(
        ORMA_EV_RCLPY_PUBLISH_ENTER,
        now(),
        &orma_ev_handle { handle: a(&ctx, 0) },
    );
    0
}
