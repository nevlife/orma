//! rclcpp C++ functions (design 2.1.2): which group a callback is in, which executor runs a
//! group, and the intra-process wiring. These read no struct offsets: `this` is the first argument
//! and a `shared_ptr` argument is the address of the caller's `shared_ptr`, whose first word is
//! the object. An argument no tracepoint has named is dropped (roscope's rule), so a changed
//! signature loses links instead of making wrong ones. Only `rclcpp_execute_*`, for processes
//! armod attached to late, reads members at measured offsets (2.1.4).

use armo_common::{bpf::*, events::*};
use aya_ebpf::{macros::uprobe, programs::ProbeContext};

use crate::{emit::*, maps::*};

fn a(ctx: &ProbeContext, n: usize) -> u64 {
    ctx.arg::<u64>(n).unwrap_or(0)
}

fn object(event: u16, object: u64, arg: u64) {
    emit(event, now(), &armo_ev_rclcpp_object { object, arg }, []);
}

fn unvalidated() -> u32 {
    lost(LOST_UNVALIDATED);
    0
}

// PublisherBase::PublisherBase(...)
#[uprobe]
pub fn rclcpp_publisher_base(ctx: ProbeContext) -> u32 {
    let this = a(&ctx, 0);
    mark(KNOWN_RCLCPP_PUB, this);
    object(ARMO_EV_RCLCPP_PUBLISHER_BASE, this, 0);
    0
}

// PublisherBase::setup_intra_process(uint64_t intra_process_publisher_id, shared_ptr<IPM>)
#[uprobe]
pub fn rclcpp_publisher_setup_intra(ctx: ProbeContext) -> u32 {
    let this = a(&ctx, 0);
    if !known(KNOWN_RCLCPP_PUB, this) {
        return unvalidated();
    }
    object(ARMO_EV_RCLCPP_PUBLISHER_SETUP_INTRA, this, a(&ctx, 1));
    0
}

// SubscriptionBase::setup_intra_process(uint64_t intra_process_subscription_id, weak_ptr<IPM>)
#[uprobe]
pub fn rclcpp_subscription_setup_intra(ctx: ProbeContext) -> u32 {
    let this = a(&ctx, 0);
    if !known(KNOWN_RCLCPP_SUB, this) {
        return unvalidated();
    }
    object(ARMO_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA, this, a(&ctx, 1));
    0
}

// IntraProcessManager::insert_sub_id_for_pub(sub_id, pub_id, use_take_shared_method)
#[uprobe]
pub fn rclcpp_ipm_insert_sub_id_for_pub(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_rclcpp_ipm_insert_sub_id_for_pub {
        ipm: a(&ctx, 0),
        sub_id: a(&ctx, 1),
        pub_id: a(&ctx, 2),
        use_take_shared_method: (a(&ctx, 3) & 0xff != 0) as u32,
        pad: 0,
    };
    if ev.sub_id == 0 || ev.pub_id == 0 {
        return unvalidated();
    }
    emit(ARMO_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB, now(), &ev, []);
    0
}

// CallbackGroup::add_subscription(shared_ptr<SubscriptionBase>)
#[uprobe]
pub fn rclcpp_group_add_subscription(ctx: ProbeContext) -> u32 {
    let group = a(&ctx, 0);
    let sub = read_u64(a(&ctx, 1));
    if !known(KNOWN_RCLCPP_SUB, sub) {
        return unvalidated();
    }
    mark(LINKED_RCLCPP_SUB, sub);
    object(ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION, group, sub);
    0
}

// CallbackGroup::add_timer(shared_ptr<TimerBase>). The timer is a TimerBase, which no tracepoint
// names; the model takes the rcl handle from the rclcpp_timer_link_node that follows on this
// thread.
#[uprobe]
pub fn rclcpp_group_add_timer(ctx: ProbeContext) -> u32 {
    object(
        ARMO_EV_RCLCPP_GROUP_ADD_TIMER,
        a(&ctx, 0),
        read_u64(a(&ctx, 1)),
    );
    0
}

// Executor::Executor(const ExecutorOptions &) and Executor(const shared_ptr<Context> &)
#[uprobe]
pub fn rclcpp_executor(ctx: ProbeContext) -> u32 {
    let this = a(&ctx, 0);
    mark(KNOWN_EXECUTOR, this);
    object(ARMO_EV_RCLCPP_EXECUTOR, this, 0);
    0
}

// ExecutorEntitiesCollector::ExecutorEntitiesCollector(shared_ptr<...>): runs inside the
// executor's constructor on the same thread.
#[uprobe]
pub fn rclcpp_collector(ctx: ProbeContext) -> u32 {
    let this = a(&ctx, 0);
    mark(KNOWN_COLLECTOR, this);
    object(ARMO_EV_RCLCPP_COLLECTOR, this, read_u64(a(&ctx, 1)));
    0
}

/// A group added to an executor or its collector. The owner must have been constructed while
/// armod watched, or the link names an object no other event names.
fn group_to(ctx: &ProbeContext, owner: u32, event: u16) -> u32 {
    let this = a(ctx, 0);
    let group = read_u64(a(ctx, 1));
    if group == 0 || !known(owner, this) {
        return unvalidated();
    }
    object(event, this, group);
    0
}

// ExecutorEntitiesCollector::add_callback_group_to_collection(shared_ptr<CallbackGroup>, ...)
#[uprobe]
pub fn rclcpp_collector_add_group_to_collection(ctx: ProbeContext) -> u32 {
    group_to(
        &ctx,
        KNOWN_COLLECTOR,
        ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION,
    )
}

// ExecutorEntitiesCollector::add_callback_group(shared_ptr<CallbackGroup>)
#[uprobe]
pub fn rclcpp_collector_add_group(ctx: ProbeContext) -> u32 {
    group_to(&ctx, KNOWN_COLLECTOR, ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP)
}

// rclcpp 16 (Humble): Executor::add_callback_group_to_map(shared_ptr<CallbackGroup>, ...)
#[uprobe]
pub fn rclcpp_executor_add_group(ctx: ProbeContext) -> u32 {
    group_to(&ctx, KNOWN_EXECUTOR, ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP)
}

// Offsets measured on Humble, Jazzy and Lyrical (design 2.1.4). The `shared_ptr` members' first
// word is the object.
const ANY_SUBSCRIPTION: u64 = 8;
const ANY_TIMER: u64 = 24;
const ANY_SERVICE: u64 = 40;
const ANY_GROUP: u64 = 88;
const SUBSCRIPTION_NODE: u64 = 32;
const SERVICE_NODE: u64 = 8;
const CLIENT_NODE: u64 = 24;
const GROUP_SUBSCRIPTIONS: u64 = 64;
const GROUP_SERVICES: u64 = 112;
const GROUP_CLIENTS: u64 = 136;

/// The rcl handle members, which moved in rclcpp 28.
struct Handles {
    subscription: u64,
    service: u64,
    timer: u64,
}

const HANDLES_16: Handles = Handles {
    subscription: 48,
    service: 24,
    timer: 24,
};
const HANDLES_28: Handles = Handles {
    subscription: 128,
    service: 104,
    timer: 104,
};

/// A timer holds no node: take the node of the first subscription, service or client in its
/// callback group, which belongs to one node.
fn group_node(group: u64) -> u64 {
    for (member, node) in [
        (GROUP_SUBSCRIPTIONS, SUBSCRIPTION_NODE),
        (GROUP_SERVICES, SERVICE_NODE),
        (GROUP_CLIENTS, CLIENT_NODE),
    ] {
        let begin = read_u64(group + member);
        if begin != 0 && begin != read_u64(group + member + 8) {
            let object = read_u64(begin);
            if object != 0 {
                return read_u64(object + node);
            }
        }
    }
    0
}

/// Notes on the thread what the executor is about to run; `callback_start` takes it. Every run
/// overwrites it, so a take that finds no message leaves nothing behind.
fn execute(ctx: &ProbeContext, h: &Handles) -> u32 {
    let exec = a(ctx, 1);
    let subscription = read_u64(exec + ANY_SUBSCRIPTION);
    let service = read_u64(exec + ANY_SERVICE);
    let timer = read_u64(exec + ANY_TIMER);
    let (kind, object, handle, node) = if subscription != 0 {
        (
            ARMO_OWNER_SUBSCRIPTION,
            subscription,
            read_u64(subscription + h.subscription),
            read_u64(subscription + SUBSCRIPTION_NODE),
        )
    } else if service != 0 {
        let handle = read_u64(service + h.service);
        let node = read_u64(service + SERVICE_NODE);
        (ARMO_OWNER_SERVICE, handle, handle, node)
    } else if timer != 0 {
        let handle = read_u64(timer + h.timer);
        let node = group_node(read_u64(exec + ANY_GROUP));
        (ARMO_OWNER_TIMER, handle, handle, node)
    } else {
        (0, 0, 0, 0)
    };
    let Some(t) = thread() else {
        return 0;
    };
    // SAFETY: the entry is this thread's own; only its own uprobes write it.
    unsafe {
        (*t).owner_kind = kind;
        (*t).owner_depth = (*t).depth;
        (*t).owner_object = object;
        (*t).owner_handle = handle;
        (*t).owner_node = node;
    }
    0
}

// rclcpp 16: Executor::execute_any_executable(AnyExecutable &)
#[uprobe]
pub fn rclcpp_execute_16(ctx: ProbeContext) -> u32 {
    execute(&ctx, &HANDLES_16)
}

// rclcpp 28 and later
#[uprobe]
pub fn rclcpp_execute_28(ctx: ProbeContext) -> u32 {
    execute(&ctx, &HANDLES_28)
}
