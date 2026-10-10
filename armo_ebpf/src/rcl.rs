//! librcl.so functions without a trace function (design 2.1.2): the wait and the timer's end.

use armo_common::{events::*, shm::ARMO_SRC_ROS};
use aya_ebpf::{
    macros::{uprobe, uretprobe},
    programs::{ProbeContext, RetProbeContext},
};

use crate::{emit::*, maps::*};

// rcl_ret_t rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout)
#[uprobe]
pub fn rcl_wait(ctx: ProbeContext) -> u32 {
    let ns = now();
    let wait_set = ctx.arg::<u64>(0).unwrap_or(0);
    wait_enter(ARMO_SRC_ROS, wait_set, ns);
    if streamed(tgid()) {
        let ev = armo_ev_rcl_wait_enter {
            wait_set,
            timeout: ctx.arg::<i64>(1).unwrap_or(0),
        };
        emit(ARMO_EV_RCL_WAIT_ENTER, ns, &ev, []);
    }
    0
}

#[uretprobe]
pub fn rcl_wait_ret(ctx: RetProbeContext) -> u32 {
    let ns = now();
    if !mine(&ctx) {
        return 0;
    }
    let wait_set = wake(ARMO_SRC_ROS, None, ns);
    if streamed(tgid()) {
        let ev = armo_ev_rcl_wait_exit {
            wait_set,
            ret: ctx.ret::<i32>(),
            pad: 0,
        };
        emit(ARMO_EV_RCL_WAIT_EXIT, ns, &ev, []);
    }
    0
}

// rcl_ret_t rcl_timer_fini(rcl_timer_t * timer)
#[uprobe]
pub fn rcl_timer_fini(ctx: ProbeContext) -> u32 {
    let ev = armo_ev_handle {
        handle: ctx.arg::<u64>(0).unwrap_or(0),
    };
    emit(ARMO_EV_RCL_TIMER_FINI, now(), &ev, []);
    0
}
