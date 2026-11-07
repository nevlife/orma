//! Per event: its name, payload size, string count, and whether the `LD_PRELOAD` and `LTTng` backends
//! receive it through `orma_emit_<name>` (`orma_preload/include/orma_preload/orma_emit.h`).
//! Readers check records against it (design 13.6).

use crate::events::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventInfo {
    pub id: u16,
    pub name: &'static str,
    /// The payload type's name, the same in Rust and C; `()` for none.
    pub payload: &'static str,
    pub size: usize,
    pub strings: u8,
    pub emitted: bool,
}

impl EventInfo {
    /// Bounds of a valid record's `len`: every string at least its NUL, at most `ORMA_STRING_MAX`.
    pub const fn len_range(&self) -> (usize, usize) {
        let fixed = size_of::<orma_record>() + self.size;
        let s = self.strings as usize;
        (
            (fixed + s).next_multiple_of(ORMA_RECORD_ALIGN),
            (fixed + s * ORMA_STRING_MAX).next_multiple_of(ORMA_RECORD_ALIGN),
        )
    }
}

macro_rules! ev {
    ($id:ident, $name:literal, $ty:ty, $strings:literal, $emitted:literal) => {
        EventInfo {
            id: $id,
            name: $name,
            payload: stringify!($ty),
            size: size_of::<$ty>(),
            strings: $strings,
            emitted: $emitted,
        }
    };
}

/// Events that name and link entities: `liborma_preload.so` writes them before its file is armed,
/// and orma-collectd keeps them for recordings that start later. `init_event` in
/// `orma_preload/src/shm_backend.c` is the same list.
pub const fn init(event: u16) -> bool {
    matches!(
        event,
        ORMA_EV_RCL_INIT
            | ORMA_EV_RCL_NODE_INIT
            | ORMA_EV_RMW_PUBLISHER_INIT
            | ORMA_EV_RCL_PUBLISHER_INIT
            | ORMA_EV_RMW_SUBSCRIPTION_INIT
            | ORMA_EV_RCL_SUBSCRIPTION_INIT
            | ORMA_EV_RCLCPP_SUBSCRIPTION_INIT
            | ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED
            | ORMA_EV_RCL_SERVICE_INIT
            | ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED
            | ORMA_EV_RCL_CLIENT_INIT
            | ORMA_EV_RCL_TIMER_INIT
            | ORMA_EV_RCLCPP_TIMER_CALLBACK_ADDED
            | ORMA_EV_RCLCPP_TIMER_LINK_NODE
            | ORMA_EV_RCLCPP_CALLBACK_REGISTER
            | ORMA_EV_RCL_TIMER_FINI
            | ORMA_EV_RCLCPP_PUBLISHER_BASE
            | ORMA_EV_RCLCPP_PUBLISHER_SETUP_INTRA
            | ORMA_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA
            | ORMA_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB
            | ORMA_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION
            | ORMA_EV_RCLCPP_GROUP_ADD_TIMER
            | ORMA_EV_RCLCPP_EXECUTOR
            | ORMA_EV_RCLCPP_COLLECTOR
            | ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION
            | ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP
            | ORMA_EV_RCLCPP_EXECUTOR_ADD_GROUP
            | ORMA_EV_RCLCPP_CALLBACK_OWNER
            | ORMA_EV_CALLBACK_CODE
            | ORMA_EV_FLUX_CHANNEL_INIT
            | ORMA_EV_FLUX_ENDPOINT_INIT
            | ORMA_EV_FLUX_GPU_WEDGED
    )
}

pub const CATALOG: &[EventInfo] = &[
    ev!(ORMA_EV_RCL_INIT, "rcl_init", orma_ev_rcl_init, 0, true),
    ev!(
        ORMA_EV_RCL_NODE_INIT,
        "rcl_node_init",
        orma_ev_rcl_node_init,
        2,
        true
    ),
    ev!(
        ORMA_EV_RMW_PUBLISHER_INIT,
        "rmw_publisher_init",
        orma_ev_rmw_endpoint_init,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_PUBLISHER_INIT,
        "rcl_publisher_init",
        orma_ev_rcl_topic_init,
        1,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_PUBLISH,
        "rclcpp_publish",
        orma_ev_publish,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_INTRA_PUBLISH,
        "rclcpp_intra_publish",
        orma_ev_publish,
        0,
        true
    ),
    ev!(ORMA_EV_RCL_PUBLISH, "rcl_publish", orma_ev_publish, 0, true),
    ev!(
        ORMA_EV_RMW_PUBLISH,
        "rmw_publish",
        orma_ev_rmw_publish,
        0,
        true
    ),
    ev!(
        ORMA_EV_RMW_PUBLISH_RCL,
        "rmw_publish_rcl",
        orma_ev_rmw_publish_rcl,
        0,
        true
    ),
    ev!(
        ORMA_EV_RMW_SUBSCRIPTION_INIT,
        "rmw_subscription_init",
        orma_ev_rmw_endpoint_init,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_SUBSCRIPTION_INIT,
        "rcl_subscription_init",
        orma_ev_rcl_topic_init,
        1,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_SUBSCRIPTION_INIT,
        "rclcpp_subscription_init",
        orma_ev_rclcpp_subscription_init,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
        "rclcpp_subscription_callback_added",
        orma_ev_callback_added,
        0,
        true
    ),
    ev!(ORMA_EV_RMW_TAKE, "rmw_take", orma_ev_rmw_take, 0, true),
    ev!(ORMA_EV_RCL_TAKE, "rcl_take", orma_ev_take, 0, true),
    ev!(ORMA_EV_RCLCPP_TAKE, "rclcpp_take", orma_ev_take, 0, true),
    ev!(
        ORMA_EV_RCL_SERVICE_INIT,
        "rcl_service_init",
        orma_ev_rcl_service_init,
        1,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED,
        "rclcpp_service_callback_added",
        orma_ev_callback_added,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_CLIENT_INIT,
        "rcl_client_init",
        orma_ev_rcl_service_init,
        1,
        true
    ),
    ev!(
        ORMA_EV_RCL_TIMER_INIT,
        "rcl_timer_init",
        orma_ev_rcl_timer_init,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_TIMER_CALLBACK_ADDED,
        "rclcpp_timer_callback_added",
        orma_ev_callback_added,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_TIMER_LINK_NODE,
        "rclcpp_timer_link_node",
        orma_ev_rclcpp_timer_link_node,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_CALLBACK_REGISTER,
        "rclcpp_callback_register",
        orma_ev_rclcpp_callback_register,
        1,
        true
    ),
    ev!(
        ORMA_EV_CALLBACK_START,
        "callback_start",
        orma_ev_callback_start,
        0,
        true
    ),
    ev!(
        ORMA_EV_CALLBACK_END,
        "callback_end",
        orma_ev_handle,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_EXECUTOR_GET_NEXT_READY,
        "rclcpp_executor_get_next_ready",
        (),
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK,
        "rclcpp_executor_wait_for_work",
        orma_ev_rclcpp_executor_wait_for_work,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_EXECUTOR_EXECUTE,
        "rclcpp_executor_execute",
        orma_ev_handle,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_WAIT_ENTER,
        "rcl_wait_enter",
        orma_ev_rcl_wait_enter,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_WAIT_EXIT,
        "rcl_wait_exit",
        orma_ev_rcl_wait_exit,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_TIMER_FINI,
        "rcl_timer_fini",
        orma_ev_handle,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_PUBLISHER_BASE,
        "rclcpp_publisher_base",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_PUBLISHER_SETUP_INTRA,
        "rclcpp_publisher_setup_intra",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA,
        "rclcpp_subscription_setup_intra",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB,
        "rclcpp_ipm_insert_sub_id_for_pub",
        orma_ev_rclcpp_ipm_insert_sub_id_for_pub,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION,
        "rclcpp_group_add_subscription",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_GROUP_ADD_TIMER,
        "rclcpp_group_add_timer",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_EXECUTOR,
        "rclcpp_executor",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_COLLECTOR,
        "rclcpp_collector",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION,
        "rclcpp_collector_add_group_to_collection",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP,
        "rclcpp_collector_add_group",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_CALLBACK_CODE,
        "callback_code",
        orma_ev_callback_code,
        0,
        true
    ),
    ev!(
        ORMA_EV_RMW_SUBSCRIPTION_TOPIC,
        "rmw_subscription_topic",
        orma_ev_rmw_topic,
        1,
        false
    ),
    ev!(
        ORMA_EV_RMW_PUBLISHER_TOPIC,
        "rmw_publisher_topic",
        orma_ev_rmw_topic,
        1,
        false
    ),
    ev!(
        ORMA_EV_DDS_SERIALIZED_SIZE,
        "dds_serialized_size",
        orma_ev_dds_serialized_size,
        0,
        false
    ),
    ev!(
        ORMA_EV_RCLCPP_EXECUTOR_ADD_GROUP,
        "rclcpp_executor_add_group",
        orma_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLCPP_CALLBACK_OWNER,
        "rclcpp_callback_owner",
        orma_ev_rclcpp_callback_owner,
        1,
        false
    ),
    ev!(ORMA_EV_DDS_WRITE, "dds_write", orma_ev_dds_write, 0, true),
    ev!(
        ORMA_EV_RCL_EXECUTOR_CLAIM,
        "rcl_executor_claim",
        orma_ev_rcl_executor_claim,
        0,
        false
    ),
    ev!(
        ORMA_EV_RCL_EXECUTOR_COMPLETE,
        "rcl_executor_complete",
        orma_ev_rcl_executor_complete,
        0,
        false
    ),
    ev!(
        ORMA_EV_RCL_IPC_PUBLISH,
        "rcl_ipc_publish",
        orma_ev_rcl_ipc_publish,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_IPC_TAKE,
        "rcl_ipc_take",
        orma_ev_rcl_ipc_take,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_IPC_RELEASE,
        "rcl_ipc_release",
        orma_ev_handle,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_LOAN_BORROW,
        "rcl_loan_borrow",
        orma_ev_rcl_loan,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_LOAN_PUBLISH,
        "rcl_loan_publish",
        orma_ev_rcl_loan,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCL_LOAN_TAKE,
        "rcl_loan_take",
        orma_ev_rcl_loan,
        0,
        true
    ),
    ev!(
        ORMA_EV_RMW_LOAN_RETURN,
        "rmw_loan_return",
        orma_ev_rcl_loan,
        0,
        true
    ),
    ev!(
        ORMA_EV_ZENOH_SHM_PAYLOAD,
        "zenoh_shm_payload",
        orma_ev_handle,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLPY_TAKE_CONVERTED,
        "rclpy_take_converted",
        orma_ev_rclpy_take,
        0,
        true
    ),
    ev!(
        ORMA_EV_RCLPY_PUBLISH_ENTER,
        "rclpy_publish_enter",
        orma_ev_handle,
        0,
        true
    ),
    ev!(ORMA_EV_CUDA_CALL, "cuda_call", orma_ev_cuda_call, 0, true),
    ev!(ORMA_EV_SPAN_BEGIN, "span_begin", orma_ev_span, 1, true),
    ev!(ORMA_EV_SPAN_END, "span_end", orma_ev_span, 0, true),
    ev!(
        ORMA_EV_FLUX_CHANNEL_INIT,
        "flux_channel_init",
        orma_ev_flux_channel_init,
        1,
        true
    ),
    ev!(
        ORMA_EV_FLUX_CLAIM,
        "flux_claim",
        orma_ev_flux_claim,
        0,
        true
    ),
    ev!(ORMA_EV_FLUX_LOAN, "flux_loan", orma_ev_flux_loan, 0, true),
    ev!(
        ORMA_EV_FLUX_LOAN_END,
        "flux_loan_end",
        orma_ev_flux_loan_end,
        0,
        true
    ),
    ev!(
        ORMA_EV_FLUX_COMMIT,
        "flux_commit",
        orma_ev_flux_commit,
        0,
        true
    ),
    ev!(ORMA_EV_FLUX_TAKE, "flux_take", orma_ev_flux_take, 0, true),
    ev!(
        ORMA_EV_FLUX_RELEASE,
        "flux_release",
        orma_ev_flux_release,
        0,
        true
    ),
    ev!(
        ORMA_EV_FLUX_REFUSED,
        "flux_refused",
        orma_ev_flux_refused,
        0,
        true
    ),
    ev!(ORMA_EV_FLUX_WAIT, "flux_wait", orma_ev_flux_wait, 0, true),
    ev!(ORMA_EV_FLUX_WAKE, "flux_wake", orma_ev_flux_wake, 0, true),
    ev!(
        ORMA_EV_FLUX_CALLBACK_START,
        "flux_callback_start",
        orma_ev_flux_callback_start,
        0,
        true
    ),
    ev!(
        ORMA_EV_FLUX_CALLBACK_END,
        "flux_callback_end",
        orma_ev_flux_callback_end,
        1,
        true
    ),
    ev!(
        ORMA_EV_FLUX_ENDPOINT_INIT,
        "flux_endpoint_init",
        orma_ev_flux_endpoint_init,
        3,
        true
    ),
    ev!(
        ORMA_EV_FLUX_PY_GIL,
        "flux_py_gil",
        orma_ev_flux_py_gil,
        0,
        true
    ),
    ev!(
        ORMA_EV_FLUX_PY_READY,
        "flux_py_ready",
        orma_ev_flux_py_ready,
        0,
        true
    ),
    ev!(
        ORMA_EV_FLUX_RECORD_DROP,
        "flux_record_drop",
        orma_ev_flux_record_drop,
        0,
        true
    ),
    ev!(
        ORMA_EV_FLUX_GPU_WEDGED,
        "flux_gpu_wedged",
        orma_ev_flux_gpu_wedged,
        0,
        true
    ),
    ev!(
        ORMA_EV_SCHED_SWITCH,
        "sched_switch",
        orma_ev_sched_switch,
        0,
        false
    ),
    ev!(
        ORMA_EV_SCHED_WAKEUP,
        "sched_wakeup",
        orma_ev_sched_wakeup,
        0,
        false
    ),
    ev!(
        ORMA_EV_FUTEX_WAIT,
        "futex_wait",
        orma_ev_futex_wait,
        0,
        false
    ),
];

pub fn info(id: u16) -> Option<&'static EventInfo> {
    CATALOG.iter().find(|e| e.id == id)
}
