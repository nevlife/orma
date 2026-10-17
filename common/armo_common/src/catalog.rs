//! Per event: its name, payload size, string count, and whether the LD_PRELOAD and LTTng backends
//! receive it through `armo_emit_<name>` (`armo_preload/include/armo_preload/armo_emit.h`).
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
    /// Bounds of a valid record's `len`: every string at least its NUL, at most `ARMO_STRING_MAX`.
    pub const fn len_range(&self) -> (usize, usize) {
        let fixed = size_of::<armo_record>() + self.size;
        let s = self.strings as usize;
        (
            (fixed + s).next_multiple_of(ARMO_RECORD_ALIGN),
            (fixed + s * ARMO_STRING_MAX).next_multiple_of(ARMO_RECORD_ALIGN),
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

/// Events that name and link entities: libarmo_preload.so writes them before its file is armed,
/// and armo-collectd keeps them for recordings that start later. `init_event` in
/// `armo_preload/src/shm_backend.c` is the same list.
pub const fn init(event: u16) -> bool {
    matches!(
        event,
        ARMO_EV_RCL_INIT
            | ARMO_EV_RCL_NODE_INIT
            | ARMO_EV_RMW_PUBLISHER_INIT
            | ARMO_EV_RCL_PUBLISHER_INIT
            | ARMO_EV_RMW_SUBSCRIPTION_INIT
            | ARMO_EV_RCL_SUBSCRIPTION_INIT
            | ARMO_EV_RCLCPP_SUBSCRIPTION_INIT
            | ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED
            | ARMO_EV_RCL_SERVICE_INIT
            | ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED
            | ARMO_EV_RCL_CLIENT_INIT
            | ARMO_EV_RCL_TIMER_INIT
            | ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED
            | ARMO_EV_RCLCPP_TIMER_LINK_NODE
            | ARMO_EV_RCLCPP_CALLBACK_REGISTER
            | ARMO_EV_RMW_CLIENT_INIT
            | ARMO_EV_RCL_TIMER_FINI
            | ARMO_EV_RCLCPP_PUBLISHER_BASE
            | ARMO_EV_RCLCPP_PUBLISHER_SETUP_INTRA
            | ARMO_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA
            | ARMO_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB
            | ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION
            | ARMO_EV_RCLCPP_GROUP_ADD_TIMER
            | ARMO_EV_RCLCPP_EXECUTOR
            | ARMO_EV_RCLCPP_COLLECTOR
            | ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION
            | ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP
            | ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP
            | ARMO_EV_RCLCPP_CALLBACK_OWNER
            | ARMO_EV_CALLBACK_CODE
            | ARMO_EV_FLUX2_CHANNEL_INIT
            | ARMO_EV_FLUX2_ENDPOINT_INIT
            | ARMO_EV_FLUX2_GPU_WEDGED
    )
}

pub const CATALOG: &[EventInfo] = &[
    ev!(ARMO_EV_RCL_INIT, "rcl_init", armo_ev_rcl_init, 0, true),
    ev!(
        ARMO_EV_RCL_NODE_INIT,
        "rcl_node_init",
        armo_ev_rcl_node_init,
        2,
        true
    ),
    ev!(
        ARMO_EV_RMW_PUBLISHER_INIT,
        "rmw_publisher_init",
        armo_ev_rmw_endpoint_init,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCL_PUBLISHER_INIT,
        "rcl_publisher_init",
        armo_ev_rcl_topic_init,
        1,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_PUBLISH,
        "rclcpp_publish",
        armo_ev_publish,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_INTRA_PUBLISH,
        "rclcpp_intra_publish",
        armo_ev_publish,
        0,
        true
    ),
    ev!(ARMO_EV_RCL_PUBLISH, "rcl_publish", armo_ev_publish, 0, true),
    ev!(
        ARMO_EV_RMW_PUBLISH,
        "rmw_publish",
        armo_ev_rmw_publish,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_PUBLISH_RCL,
        "rmw_publish_rcl",
        armo_ev_rmw_publish_rcl,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_SUBSCRIPTION_INIT,
        "rmw_subscription_init",
        armo_ev_rmw_endpoint_init,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCL_SUBSCRIPTION_INIT,
        "rcl_subscription_init",
        armo_ev_rcl_topic_init,
        1,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_SUBSCRIPTION_INIT,
        "rclcpp_subscription_init",
        armo_ev_rclcpp_subscription_init,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
        "rclcpp_subscription_callback_added",
        armo_ev_callback_added,
        0,
        true
    ),
    ev!(ARMO_EV_RMW_TAKE, "rmw_take", armo_ev_rmw_take, 0, true),
    ev!(ARMO_EV_RCL_TAKE, "rcl_take", armo_ev_take, 0, true),
    ev!(ARMO_EV_RCLCPP_TAKE, "rclcpp_take", armo_ev_take, 0, true),
    ev!(
        ARMO_EV_RCL_SERVICE_INIT,
        "rcl_service_init",
        armo_ev_rcl_service_init,
        1,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED,
        "rclcpp_service_callback_added",
        armo_ev_callback_added,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCL_CLIENT_INIT,
        "rcl_client_init",
        armo_ev_rcl_service_init,
        1,
        true
    ),
    ev!(
        ARMO_EV_RCL_TIMER_INIT,
        "rcl_timer_init",
        armo_ev_rcl_timer_init,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED,
        "rclcpp_timer_callback_added",
        armo_ev_callback_added,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_TIMER_LINK_NODE,
        "rclcpp_timer_link_node",
        armo_ev_rclcpp_timer_link_node,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_CALLBACK_REGISTER,
        "rclcpp_callback_register",
        armo_ev_rclcpp_callback_register,
        1,
        true
    ),
    ev!(
        ARMO_EV_CALLBACK_START,
        "callback_start",
        armo_ev_callback_start,
        0,
        true
    ),
    ev!(
        ARMO_EV_CALLBACK_END,
        "callback_end",
        armo_ev_handle,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_EXECUTOR_GET_NEXT_READY,
        "rclcpp_executor_get_next_ready",
        (),
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK,
        "rclcpp_executor_wait_for_work",
        armo_ev_rclcpp_executor_wait_for_work,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_EXECUTOR_EXECUTE,
        "rclcpp_executor_execute",
        armo_ev_handle,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_CLIENT_INIT,
        "rmw_client_init",
        armo_ev_rmw_endpoint_init,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_SEND_REQUEST,
        "rmw_send_request",
        armo_ev_rmw_send_request,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_TAKE_REQUEST,
        "rmw_take_request",
        armo_ev_rmw_take_request,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_SEND_RESPONSE,
        "rmw_send_response",
        armo_ev_rmw_send_response,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_TAKE_RESPONSE,
        "rmw_take_response",
        armo_ev_rmw_take_response,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCL_WAIT_ENTER,
        "rcl_wait_enter",
        armo_ev_rcl_wait_enter,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCL_WAIT_EXIT,
        "rcl_wait_exit",
        armo_ev_rcl_wait_exit,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCL_TIMER_FINI,
        "rcl_timer_fini",
        armo_ev_handle,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_PUBLISHER_BASE,
        "rclcpp_publisher_base",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_PUBLISHER_SETUP_INTRA,
        "rclcpp_publisher_setup_intra",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA,
        "rclcpp_subscription_setup_intra",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB,
        "rclcpp_ipm_insert_sub_id_for_pub",
        armo_ev_rclcpp_ipm_insert_sub_id_for_pub,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION,
        "rclcpp_group_add_subscription",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_GROUP_ADD_TIMER,
        "rclcpp_group_add_timer",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_EXECUTOR,
        "rclcpp_executor",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_COLLECTOR,
        "rclcpp_collector",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION,
        "rclcpp_collector_add_group_to_collection",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP,
        "rclcpp_collector_add_group",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_CALLBACK_CODE,
        "callback_code",
        armo_ev_callback_code,
        0,
        true
    ),
    ev!(
        ARMO_EV_RMW_SUBSCRIPTION_TOPIC,
        "rmw_subscription_topic",
        armo_ev_rmw_topic,
        1,
        false
    ),
    ev!(
        ARMO_EV_RMW_PUBLISHER_TOPIC,
        "rmw_publisher_topic",
        armo_ev_rmw_topic,
        1,
        false
    ),
    ev!(
        ARMO_EV_DDS_SERIALIZED_SIZE,
        "dds_serialized_size",
        armo_ev_dds_serialized_size,
        0,
        false
    ),
    ev!(
        ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP,
        "rclcpp_executor_add_group",
        armo_ev_rclcpp_object,
        0,
        true
    ),
    ev!(
        ARMO_EV_RCLCPP_CALLBACK_OWNER,
        "rclcpp_callback_owner",
        armo_ev_rclcpp_callback_owner,
        1,
        false
    ),
    ev!(ARMO_EV_DDS_WRITE, "dds_write", armo_ev_dds_write, 0, true),
    ev!(ARMO_EV_CUDA_CALL, "cuda_call", armo_ev_cuda_call, 0, true),
    ev!(ARMO_EV_SPAN_BEGIN, "span_begin", armo_ev_span, 1, true),
    ev!(ARMO_EV_SPAN_END, "span_end", armo_ev_span, 0, true),
    ev!(
        ARMO_EV_FLUX2_CHANNEL_INIT,
        "flux2_channel_init",
        armo_ev_flux2_channel_init,
        1,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_CLAIM,
        "flux2_claim",
        armo_ev_flux2_claim,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_LOAN,
        "flux2_loan",
        armo_ev_flux2_loan,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_LOAN_END,
        "flux2_loan_end",
        armo_ev_flux2_loan_end,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_COMMIT,
        "flux2_commit",
        armo_ev_flux2_commit,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_TAKE,
        "flux2_take",
        armo_ev_flux2_take,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_RELEASE,
        "flux2_release",
        armo_ev_flux2_release,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_REFUSED,
        "flux2_refused",
        armo_ev_flux2_refused,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_WAIT,
        "flux2_wait",
        armo_ev_flux2_wait,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_WAKE,
        "flux2_wake",
        armo_ev_flux2_wake,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_CALLBACK_START,
        "flux2_callback_start",
        armo_ev_flux2_callback_start,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_CALLBACK_END,
        "flux2_callback_end",
        armo_ev_flux2_callback_end,
        1,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_ENDPOINT_INIT,
        "flux2_endpoint_init",
        armo_ev_flux2_endpoint_init,
        3,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_PY_GIL,
        "flux2_py_gil",
        armo_ev_flux2_py_gil,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_PY_READY,
        "flux2_py_ready",
        armo_ev_flux2_py_ready,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_RECORD_DROP,
        "flux2_record_drop",
        armo_ev_flux2_record_drop,
        0,
        true
    ),
    ev!(
        ARMO_EV_FLUX2_GPU_WEDGED,
        "flux2_gpu_wedged",
        armo_ev_flux2_gpu_wedged,
        0,
        true
    ),
    ev!(
        ARMO_EV_SCHED_SWITCH,
        "sched_switch",
        armo_ev_sched_switch,
        0,
        false
    ),
    ev!(
        ARMO_EV_SCHED_WAKEUP,
        "sched_wakeup",
        armo_ev_sched_wakeup,
        0,
        false
    ),
    ev!(
        ARMO_EV_FUTEX_WAIT,
        "futex_wait",
        armo_ev_futex_wait,
        0,
        false
    ),
];

pub fn info(id: u16) -> Option<&'static EventInfo> {
    CATALOG.iter().find(|e| e.id == id)
}
