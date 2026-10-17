//! `ros_trace_*` of libtracetools.so (design 2.1). armod attaches each program to the plain name
//! only: the gated `ros_trace_do_*` form runs only when another method has opened the gate (2.4).

use armo_common::{bpf::*, events::*, shm::ARMO_SRC_ROS};
use aya_ebpf::{helpers::bpf_probe_read_user, macros::uprobe, programs::ProbeContext};

use crate::{emit::*, maps::*};

fn a(ctx: &ProbeContext, n: usize) -> u64 {
    ctx.arg::<u64>(n).unwrap_or(0)
}

/// A C++ `bool` argument: only the low byte is defined.
fn flag(ctx: &ProbeContext, n: usize) -> u32 {
    (a(ctx, n) & 0xff != 0) as u32
}

/// armod sets `gid_len` from the process's distro and clears the bytes past it.
fn gid(ptr: u64) -> [u8; ARMO_GID_MAX] {
    // SAFETY: the kernel checks the user address; a bad one returns an error, not a fault.
    unsafe { bpf_probe_read_user(ptr as *const [u8; ARMO_GID_MAX]) }.unwrap_or([0; ARMO_GID_MAX])
}

fn hot<T: Copy + 'static>(event: u16, ns: u64, p: &T) {
    if streamed(tgid()) {
        emit(event, ns, p, []);
    }
}

/// The words at a callback handle's offsets 24, 16, 32, 40 and 8 (design 2.4), once per handle.
/// Read in the observed process's context, as `/proc/<pid>/mem` needs ptrace rights.
pub fn callback_code(callback: u64, ns: u64) {
    if callback == 0 || !first(SEEN_CODE, callback) {
        return;
    }
    let ev = armo_ev_callback_code {
        callback,
        words: [
            read_u64(callback + 24),
            read_u64(callback + 16),
            read_u64(callback + 32),
            read_u64(callback + 40),
            read_u64(callback + 8),
        ],
    };
    emit(ARMO_EV_CALLBACK_CODE, ns, &ev, []);
}

// `rcl_node_t::impl` and `rcl_node_impl_s::fq_name` (rcl/src/rcl/node.c), the same on Humble,
// Jazzy and Lyrical.
const NODE_IMPL: u64 = 8;
const IMPL_FQ_NAME: u64 = 176;

/// What `rclcpp_execute_*` noted for this callback, once per callback (design 2.1.4). Only a start
/// one level below the executor's run is its callback.
fn callback_owner(callback: u64, ns: u64) {
    let Some(t) = THREADS.get_ptr_mut(tid()) else {
        return;
    };
    // SAFETY: the entry is this thread's own; only its own uprobes write it.
    let ev = unsafe {
        let kind = (*t).owner_kind;
        if kind == 0 {
            return;
        }
        (*t).owner_kind = 0;
        if (*t).owner_depth + 1 != (*t).depth {
            return;
        }
        armo_ev_rclcpp_callback_owner {
            callback,
            object: (*t).owner_object,
            handle: (*t).owner_handle,
            node: (*t).owner_node,
            kind,
            pad: 0,
        }
    };
    if !first(SEEN_CALLBACK_OWNER, callback) {
        return;
    }
    let fq_name = if ev.node == 0 {
        0
    } else {
        read_u64(read_u64(ev.node + NODE_IMPL) + IMPL_FQ_NAME)
    };
    emit(ARMO_EV_RCLCPP_CALLBACK_OWNER, ns, &ev, [fq_name]);
}

/// The topic of an rmw handle a tool that attached late cannot get from init events: the third
/// field of `rmw_publisher_t` and `rmw_subscription_t` (rmw/types.h), once per handle.
fn rmw_topic(event: u16, what: u32, handle: u64, ns: u64) {
    if handle == 0 || !first(what, handle) {
        return;
    }
    let name = read_u64(handle + 16);
    if name == 0 {
        return;
    }
    emit(event, ns, &armo_ev_rmw_topic { rmw_handle: handle }, [name]);
}

// ros_trace_rcl_init(const void * context_handle)
#[uprobe]
pub fn rcl_init(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rcl_init {
        context: a(&ctx, 0),
    };
    emit(ARMO_EV_RCL_INIT, now(), &ev, []);
    0
}

// ros_trace_rcl_node_init(node_handle, rmw_handle, node_name, node_namespace)
#[uprobe]
pub fn rcl_node_init(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rcl_node_init {
        node: a(&ctx, 0),
        rmw_node: a(&ctx, 1),
    };
    emit(ARMO_EV_RCL_NODE_INIT, now(), &ev, [a(&ctx, 2), a(&ctx, 3)]);
    0
}

fn endpoint_init(ctx: &ProbeContext, event: u16) {
    let ev = armo_ev_rmw_endpoint_init {
        rmw_handle: a(ctx, 0),
        gid: gid(a(ctx, 1)),
        gid_len: 0,
        pad: 0,
    };
    emit(event, now(), &ev, []);
}

// ros_trace_rmw_publisher_init(rmw_publisher_handle, gid)
#[uprobe]
pub fn rmw_publisher_init(ctx: ProbeContext) -> u32 {
    endpoint_init(&ctx, ARMO_EV_RMW_PUBLISHER_INIT);
    0
}

// ros_trace_rmw_subscription_init(rmw_subscription_handle, gid)
#[uprobe]
pub fn rmw_subscription_init(ctx: ProbeContext) -> u32 {
    endpoint_init(&ctx, ARMO_EV_RMW_SUBSCRIPTION_INIT);
    0
}

fn topic_init(ctx: &ProbeContext, event: u16) {
    let ev = armo_ev_rcl_topic_init {
        handle: a(ctx, 0),
        node: a(ctx, 1),
        rmw_handle: a(ctx, 2),
        queue_depth: a(ctx, 4),
    };
    emit(event, now(), &ev, [a(ctx, 3)]);
}

// ros_trace_rcl_publisher_init(publisher_handle, node_handle, rmw_publisher_handle, topic_name,
//                              queue_depth)
#[uprobe]
pub fn rcl_publisher_init(ctx: ProbeContext) -> u32 {
    topic_init(&ctx, ARMO_EV_RCL_PUBLISHER_INIT);
    0
}

// ros_trace_rcl_subscription_init(same as rcl_publisher_init)
#[uprobe]
pub fn rcl_subscription_init(ctx: ProbeContext) -> u32 {
    topic_init(&ctx, ARMO_EV_RCL_SUBSCRIPTION_INIT);
    0
}

fn service_init(ctx: &ProbeContext, event: u16) {
    let ev = armo_ev_rcl_service_init {
        handle: a(ctx, 0),
        node: a(ctx, 1),
        rmw_handle: a(ctx, 2),
    };
    emit(event, now(), &ev, [a(ctx, 3)]);
}

// ros_trace_rcl_service_init(service_handle, node_handle, rmw_service_handle, service_name)
#[uprobe]
pub fn rcl_service_init(ctx: ProbeContext) -> u32 {
    service_init(&ctx, ARMO_EV_RCL_SERVICE_INIT);
    0
}

// ros_trace_rcl_client_init(client_handle, node_handle, rmw_client_handle, service_name)
#[uprobe]
pub fn rcl_client_init(ctx: ProbeContext) -> u32 {
    service_init(&ctx, ARMO_EV_RCL_CLIENT_INIT);
    0
}

// ros_trace_rclcpp_subscription_init(subscription_handle, subscription)
#[uprobe]
pub fn rclcpp_subscription_init(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rclcpp_subscription_init {
        subscription_handle: a(&ctx, 0),
        subscription: a(&ctx, 1),
    };
    mark(KNOWN_RCLCPP_SUB, ev.subscription);
    emit(ARMO_EV_RCLCPP_SUBSCRIPTION_INIT, now(), &ev, []);
    0
}

fn callback_added(ctx: &ProbeContext, event: u16) {
    let ns = now();
    let ev = armo_ev_callback_added {
        handle: a(ctx, 0),
        callback: a(ctx, 1),
    };
    emit(event, ns, &ev, []);
    callback_code(ev.callback, ns);
}

// ros_trace_rclcpp_subscription_callback_added(subscription, callback)
#[uprobe]
pub fn rclcpp_subscription_callback_added(ctx: ProbeContext) -> u32 {
    callback_added(&ctx, ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED);
    0
}

// ros_trace_rclcpp_service_callback_added(service_handle, callback)
#[uprobe]
pub fn rclcpp_service_callback_added(ctx: ProbeContext) -> u32 {
    callback_added(&ctx, ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED);
    0
}

// ros_trace_rclcpp_timer_callback_added(timer_handle, callback)
#[uprobe]
pub fn rclcpp_timer_callback_added(ctx: ProbeContext) -> u32 {
    callback_added(&ctx, ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED);
    0
}

// ros_trace_rcl_timer_init(timer_handle, period)
#[uprobe]
pub fn rcl_timer_init(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rcl_timer_init {
        timer: a(&ctx, 0),
        period: a(&ctx, 1) as i64,
    };
    emit(ARMO_EV_RCL_TIMER_INIT, now(), &ev, []);
    0
}

// ros_trace_rclcpp_timer_link_node(timer_handle, node_handle)
#[uprobe]
pub fn rclcpp_timer_link_node(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rclcpp_timer_link_node {
        timer: a(&ctx, 0),
        node: a(&ctx, 1),
    };
    emit(ARMO_EV_RCLCPP_TIMER_LINK_NODE, now(), &ev, []);
    0
}

// ros_trace_rclcpp_callback_register(callback, function_symbol). Jazzy's rclcpp calls only the
// gated do_ form, so this fires for Humble's rclcpp and for armo_rclpy (2.1.1, 2.4).
#[uprobe]
pub fn rclcpp_callback_register(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rclcpp_callback_register {
        callback: a(&ctx, 0),
    };
    emit(ARMO_EV_RCLCPP_CALLBACK_REGISTER, now(), &ev, [a(&ctx, 1)]);
    0
}

fn publish(ctx: &ProbeContext, event: u16) {
    let ns = now();
    if streamed(tgid()) {
        let message = a(ctx, 1);
        let ev = armo_ev_publish {
            publisher: a(ctx, 0),
            message,
            stamp: stamp(message),
        };
        emit(event, ns, &ev, []);
    }
}

// ros_trace_rclcpp_publish(publisher_handle, message)
#[uprobe]
pub fn rclcpp_publish(ctx: ProbeContext) -> u32 {
    publish(&ctx, ARMO_EV_RCLCPP_PUBLISH);
    0
}

// ros_trace_rclcpp_intra_publish(publisher_handle, message)
#[uprobe]
pub fn rclcpp_intra_publish(ctx: ProbeContext) -> u32 {
    publish(&ctx, ARMO_EV_RCLCPP_INTRA_PUBLISH);
    0
}

// ros_trace_rcl_publish(publisher_handle, message)
#[uprobe]
pub fn rcl_publish(ctx: ProbeContext) -> u32 {
    publish(&ctx, ARMO_EV_RCL_PUBLISH);
    0
}

// ros_trace_rmw_publish(rmw_publisher_handle, message, timestamp): the rmw ABI (Jazzy on).
#[uprobe]
pub fn rmw_publish(ctx: ProbeContext) -> u32 {
    let ns = now();
    let handle = a(&ctx, 0);
    rmw_topic(ARMO_EV_RMW_PUBLISHER_TOPIC, SEEN_PUB_TOPIC, handle, ns);
    if streamed(tgid()) {
        let message = a(&ctx, 1);
        let ev = armo_ev_rmw_publish {
            rmw_publisher: handle,
            message,
            timestamp: a(&ctx, 2) as i64,
            stamp: stamp(message),
        };
        emit(ARMO_EV_RMW_PUBLISH, ns, &ev, []);
    }
    0
}

// ros_trace_rmw_publish(message): the rcl ABI (Humble, design 2.7.1).
#[uprobe]
pub fn rmw_publish_rcl(ctx: ProbeContext) -> u32 {
    let ns = now();
    if streamed(tgid()) {
        let message = a(&ctx, 0);
        let ev = armo_ev_rmw_publish_rcl {
            message,
            stamp: stamp(message),
        };
        emit(ARMO_EV_RMW_PUBLISH_RCL, ns, &ev, []);
    }
    0
}

// ros_trace_rmw_take(rmw_subscription_handle, message, source_timestamp, taken)
#[uprobe]
pub fn rmw_take(ctx: ProbeContext) -> u32 {
    let ns = now();
    let handle = a(&ctx, 0);
    let taken = flag(&ctx, 3);
    if taken != 0 {
        rmw_topic(ARMO_EV_RMW_SUBSCRIPTION_TOPIC, SEEN_SUB_TOPIC, handle, ns);
    }
    if streamed(tgid()) {
        let message = a(&ctx, 1);
        let ev = armo_ev_rmw_take {
            rmw_subscription: handle,
            message,
            source_timestamp: a(&ctx, 2) as i64,
            stamp: if taken != 0 { stamp(message) } else { 0 },
            taken,
            pad: 0,
        };
        emit(ARMO_EV_RMW_TAKE, ns, &ev, []);
    }
    0
}

// ros_trace_rcl_take(message)
#[uprobe]
pub fn rcl_take(ctx: ProbeContext) -> u32 {
    hot(
        ARMO_EV_RCL_TAKE,
        now(),
        &armo_ev_take {
            message: a(&ctx, 0),
        },
    );
    0
}

// ros_trace_rclcpp_take(message)
#[uprobe]
pub fn rclcpp_take(ctx: ProbeContext) -> u32 {
    hot(
        ARMO_EV_RCLCPP_TAKE,
        now(),
        &armo_ev_take {
            message: a(&ctx, 0),
        },
    );
    0
}

// ros_trace_callback_start(callback, is_intra_process)
#[uprobe]
pub fn callback_start(ctx: ProbeContext) -> u32 {
    let ns = now();
    let callback = a(&ctx, 0);
    crate::emit::callback_start(ARMO_SRC_ROS, callback, 0, ns);
    callback_code(callback, ns);
    callback_owner(callback, ns);
    let ev = armo_ev_callback_start {
        callback,
        is_intra_process: flag(&ctx, 1),
        pad: 0,
    };
    hot(ARMO_EV_CALLBACK_START, ns, &ev);
    0
}

// ros_trace_callback_end(callback)
#[uprobe]
pub fn callback_end(ctx: ProbeContext) -> u32 {
    let ns = now();
    let callback = a(&ctx, 0);
    let start = crate::emit::callback_end(ARMO_SRC_ROS, callback, 0);
    if start != 0 {
        agg(
            armo_common::shm::ARMO_AGG_CALLBACK,
            ARMO_SRC_ROS,
            0,
            callback,
            tgid(),
            tid(),
            ns.saturating_sub(start),
        );
    }
    hot(
        ARMO_EV_CALLBACK_END,
        ns,
        &armo_ev_handle { handle: callback },
    );
    0
}

// ros_trace_rclcpp_executor_get_next_ready(void)
#[uprobe]
pub fn rclcpp_executor_get_next_ready(_ctx: ProbeContext) -> u32 {
    hot(ARMO_EV_RCLCPP_EXECUTOR_GET_NEXT_READY, now(), &());
    0
}

// ros_trace_rclcpp_executor_wait_for_work(timeout)
#[uprobe]
pub fn rclcpp_executor_wait_for_work(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rclcpp_executor_wait_for_work {
        timeout: a(&ctx, 0) as i64,
    };
    hot(ARMO_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK, now(), &ev);
    0
}

// ros_trace_rclcpp_executor_execute(handle)
#[uprobe]
pub fn rclcpp_executor_execute(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_handle { handle: a(&ctx, 0) };
    hot(ARMO_EV_RCLCPP_EXECUTOR_EXECUTE, now(), &ev);
    0
}
