//! rclcpp C++ functions (design 2.1.2): which group a callback is in, which executor runs a
//! group, and the intra-process wiring. No struct offsets are read: `this` is the first argument
//! and a `shared_ptr` argument is the address of the caller's `shared_ptr`, whose first word is
//! the object. An argument no tracepoint has named is dropped (roscope's rule), so a changed
//! signature loses links instead of making wrong ones.

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
