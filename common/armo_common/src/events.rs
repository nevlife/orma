//! Event numbers and records (design 13.4). cbindgen writes
//! `armo_preload/include/armo_preload/armo_events.h` from this file, so the names are the C names. Numbers are only ever added. Handles and pointers are u64
//! identifiers; `stamp` is the 8 bytes at the message (`sec` low, `nanosec` high), 0 when unread.
//! A payload's strings follow it in the order its comment names, each NUL-terminated.
#![allow(non_camel_case_types)]

pub const ARMO_EV_PAD: u16 = 0;

// ros_trace_* in libtracetools.so (design 2.1).
pub const ARMO_EV_RCL_INIT: u16 = 1;
pub const ARMO_EV_RCL_NODE_INIT: u16 = 2;
pub const ARMO_EV_RMW_PUBLISHER_INIT: u16 = 3;
pub const ARMO_EV_RCL_PUBLISHER_INIT: u16 = 4;
pub const ARMO_EV_RCLCPP_PUBLISH: u16 = 5;
pub const ARMO_EV_RCLCPP_INTRA_PUBLISH: u16 = 6;
pub const ARMO_EV_RCL_PUBLISH: u16 = 7;
pub const ARMO_EV_RMW_PUBLISH: u16 = 8;
/// `ros_trace_rmw_publish(message)` of the rcl publish ABI (Humble, design 2.7.1).
pub const ARMO_EV_RMW_PUBLISH_RCL: u16 = 9;
pub const ARMO_EV_RMW_SUBSCRIPTION_INIT: u16 = 10;
pub const ARMO_EV_RCL_SUBSCRIPTION_INIT: u16 = 11;
pub const ARMO_EV_RCLCPP_SUBSCRIPTION_INIT: u16 = 12;
pub const ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED: u16 = 13;
pub const ARMO_EV_RMW_TAKE: u16 = 14;
pub const ARMO_EV_RCL_TAKE: u16 = 15;
pub const ARMO_EV_RCLCPP_TAKE: u16 = 16;
pub const ARMO_EV_RCL_SERVICE_INIT: u16 = 17;
pub const ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED: u16 = 18;
pub const ARMO_EV_RCL_CLIENT_INIT: u16 = 19;
pub const ARMO_EV_RCL_TIMER_INIT: u16 = 20;
pub const ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED: u16 = 21;
pub const ARMO_EV_RCLCPP_TIMER_LINK_NODE: u16 = 22;
pub const ARMO_EV_RCLCPP_CALLBACK_REGISTER: u16 = 23;
pub const ARMO_EV_CALLBACK_START: u16 = 24;
pub const ARMO_EV_CALLBACK_END: u16 = 25;
/// No payload.
pub const ARMO_EV_RCLCPP_EXECUTOR_GET_NEXT_READY: u16 = 26;
pub const ARMO_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK: u16 = 27;
pub const ARMO_EV_RCLCPP_EXECUTOR_EXECUTE: u16 = 28;

// ROS points without a trace function (design 2.1.2).
pub const ARMO_EV_RCL_WAIT_ENTER: u16 = 64;
pub const ARMO_EV_RCL_WAIT_EXIT: u16 = 65;
pub const ARMO_EV_RCL_TIMER_FINI: u16 = 66;
pub const ARMO_EV_RCLCPP_PUBLISHER_BASE: u16 = 67;
pub const ARMO_EV_RCLCPP_PUBLISHER_SETUP_INTRA: u16 = 68;
pub const ARMO_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA: u16 = 69;
pub const ARMO_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB: u16 = 70;
pub const ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION: u16 = 71;
pub const ARMO_EV_RCLCPP_GROUP_ADD_TIMER: u16 = 72;
pub const ARMO_EV_RCLCPP_EXECUTOR: u16 = 73;
pub const ARMO_EV_RCLCPP_COLLECTOR: u16 = 74;
pub const ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION: u16 = 75;
pub const ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP: u16 = 76;
pub const ARMO_EV_CALLBACK_CODE: u16 = 77;
pub const ARMO_EV_RMW_SUBSCRIPTION_TOPIC: u16 = 78;
pub const ARMO_EV_RMW_PUBLISHER_TOPIC: u16 = 79;
pub const ARMO_EV_DDS_SERIALIZED_SIZE: u16 = 80;
/// rclcpp 16 (Humble) `Executor::add_callback_group_to_map`: `object` is the executor, `arg` the
/// group. Jazzy reaches the executor through the collector events instead.
pub const ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP: u16 = 81;
/// The entity and node of a callback, read from the rclcpp objects the executor runs, for
/// processes armod attached to after their entities were made (design 2.1.4).
pub const ARMO_EV_RCLCPP_CALLBACK_OWNER: u16 = 82;
/// The DDS write call under `rmw_publish`: Cyclone DDS `dds_write`, `dds_writecdr`, Fast DDS
/// `DataWriter::write` (design 20.2). Written when the call returns.
pub const ARMO_EV_DDS_WRITE: u16 = 83;

// CUDA driver API in libcuda.so (design 20.1). Written when the call returns.
pub const ARMO_EV_CUDA_CALL: u16 = 96;

// libarmo_span.so, the opt-in markers inside callbacks (design 20.5).
pub const ARMO_EV_SPAN_BEGIN: u16 = 104;
pub const ARMO_EV_SPAN_END: u16 = 105;

// flux_trace_* of flux_trace_contract_2 (design 11.2).
pub const ARMO_EV_FLUX_CHANNEL_INIT: u16 = 128;
pub const ARMO_EV_FLUX_CLAIM: u16 = 129;
pub const ARMO_EV_FLUX_LOAN: u16 = 130;
pub const ARMO_EV_FLUX_LOAN_END: u16 = 131;
pub const ARMO_EV_FLUX_COMMIT: u16 = 132;
pub const ARMO_EV_FLUX_TAKE: u16 = 133;
pub const ARMO_EV_FLUX_RELEASE: u16 = 134;
pub const ARMO_EV_FLUX_REFUSED: u16 = 135;
pub const ARMO_EV_FLUX_WAIT: u16 = 136;
pub const ARMO_EV_FLUX_WAKE: u16 = 137;
pub const ARMO_EV_FLUX_CALLBACK_START: u16 = 138;
pub const ARMO_EV_FLUX_CALLBACK_END: u16 = 139;
pub const ARMO_EV_FLUX_ENDPOINT_INIT: u16 = 140;
pub const ARMO_EV_FLUX_PY_GIL: u16 = 141;
pub const ARMO_EV_FLUX_PY_READY: u16 = 142;
pub const ARMO_EV_FLUX_RECORD_DROP: u16 = 143;
/// Once per process, when a sticky CUDA error wedges its GPU; a flux build before it has none.
pub const ARMO_EV_FLUX_GPU_WEDGED: u16 = 144;

// Kernel scheduler (design 2.3).
pub const ARMO_EV_SCHED_SWITCH: u16 = 192;
pub const ARMO_EV_SCHED_WAKEUP: u16 = 193;
/// A futex wait of a selected thread, written when the system call returns (design 20.4).
pub const ARMO_EV_FUTEX_WAIT: u16 = 194;

/// Event numbers stay below this, so `armo_shm_header.missing` has a bit for each.
pub const ARMO_EV_LIMIT: u16 = 256;

/// A string's bytes with its NUL; longer ones are cut and flagged.
pub const ARMO_STRING_MAX: usize = 255;
pub const ARMO_RECORD_ALIGN: usize = 8;
pub const ARMO_RECORD_TRUNCATED: u32 = 1;
/// "ARMOEV1\0" read as a little-endian u64.
pub const ARMO_EVFILE_MAGIC: u64 = 0x0031_5645_4F4D_5241;
pub const ARMO_GID_MAX: usize = 24;

// Values of flux's arguments (design 11.3).
pub const ARMO_FLUX_CLAIM_OK: i32 = 0;
pub const ARMO_FLUX_CLAIM_RECLAIMED: i32 = 1;
pub const ARMO_FLUX_CLAIM_BACKPRESSURE: i32 = 2;
pub const ARMO_FLUX_CLAIM_NOT_PUBLISHER: i32 = 3;
pub const ARMO_FLUX_CLAIM_WRONG_DEVICE: i32 = 4;
pub const ARMO_FLUX_CLAIM_TOO_LARGE: i32 = 5;
pub const ARMO_FLUX_CLAIM_FENCE_FAILED: i32 = 6;
pub const ARMO_FLUX_CLAIM_GPU_WEDGED: i32 = 7;
pub const ARMO_FLUX_LOAN_COMMITTED: i32 = 0;
pub const ARMO_FLUX_LOAN_ABORTED: i32 = 1;
pub const ARMO_FLUX_LOAN_TOO_LARGE: i32 = 2;
pub const ARMO_FLUX_LOAN_FENCE_FAILED: i32 = 3;
pub const ARMO_FLUX_LOAN_NOT_PUBLISHER: i32 = 4;
pub const ARMO_FLUX_LOAN_GPU_WEDGED: i32 = 5;
pub const ARMO_FLUX_WAKE_TIMEOUT: i32 = 0;
pub const ARMO_FLUX_WAKE_SOURCE: i32 = 1;
pub const ARMO_FLUX_WAKE_CONTROL: i32 = 2;
pub const ARMO_FLUX_WAKE_ALREADY: i32 = 3;
/// Only from flux builds before EINTR was retried; later ones keep waiting after a signal.
pub const ARMO_FLUX_WAKE_SIGNAL: i32 = 4;
pub const ARMO_FLUX_REFUSED_MAX_BORROW: i32 = 0;
pub const ARMO_FLUX_REFUSED_NO_READER: i32 = 1;
pub const ARMO_FLUX_REFUSED_CONTENDED: i32 = 2;
pub const ARMO_FLUX_REFUSED_BAD_FRAME: i32 = 3;
pub const ARMO_FLUX_REFUSED_FENCE: i32 = 4;
pub const ARMO_FLUX_REFUSED_GPU_WEDGED: i32 = 5;
pub const ARMO_FLUX_RECORD_CACHE_FULL: i32 = 0;
pub const ARMO_FLUX_RECORD_WRITE_FAILED: i32 = 1;
pub const ARMO_FLUX_RECORD_OVERRUN: i32 = 2;
pub const ARMO_FLUX_ROLE_PUBLISHER: i32 = 0;
pub const ARMO_FLUX_ROLE_SUBSCRIPTION: i32 = 1;
pub const ARMO_FLUX_LANG_CPP: i32 = 0;
pub const ARMO_FLUX_LANG_PYTHON: i32 = 1;
pub const ARMO_FLUX_NO_SLOT: u32 = u32::MAX;
pub const ARMO_FLUX_NO_ENTRY: u32 = u32::MAX;

// Values of `armo_ev_cuda_call.api` (design 20.1).
pub const ARMO_CUDA_LAUNCH: u32 = 0;
pub const ARMO_CUDA_GRAPH_LAUNCH: u32 = 1;
pub const ARMO_CUDA_MEMCPY: u32 = 2;
pub const ARMO_CUDA_MEMCPY_ASYNC: u32 = 3;
pub const ARMO_CUDA_SYNC_STREAM: u32 = 4;
pub const ARMO_CUDA_SYNC_CTX: u32 = 5;
pub const ARMO_CUDA_SYNC_EVENT: u32 = 6;
pub const ARMO_CUDA_ALLOC: u32 = 7;
pub const ARMO_CUDA_FREE: u32 = 8;
pub const ARMO_CUDA_API_COUNT: u32 = 9;

/// `armo_ev_futex_wait.op` of a `futex_waitv` call, whose `uaddr` is the first waiter's.
pub const ARMO_FUTEX_WAITV: u32 = u32::MAX;

/// `len` counts this header, the payload and the strings, rounded up to `ARMO_RECORD_ALIGN`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_record {
    pub event: u16,
    pub len: u16,
    pub flags: u32,
    pub t_mono_ns: u64,
}

/// Precedes a record in an event batch and an `.armoev` file; not counted in its `len`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_position {
    pub pid: u32,
    pub tid: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_evfile_header {
    pub magic: u64,
    /// `ARMO_SHM_VERSION`.
    pub version: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_init {
    pub context: u64,
}

/// Strings: node name, namespace.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_node_init {
    pub node: u64,
    pub rmw_node: u64,
}

/// `gid_len` is the distro's `RMW_GID_STORAGE_SIZE`; the rest of `gid` is zero.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rmw_endpoint_init {
    pub rmw_handle: u64,
    pub gid: [u8; ARMO_GID_MAX],
    pub gid_len: u32,
    pub pad: u32,
}

/// Strings: topic. Also `ARMO_EV_RCL_SUBSCRIPTION_INIT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_topic_init {
    pub handle: u64,
    pub node: u64,
    pub rmw_handle: u64,
    pub queue_depth: u64,
}

/// `ARMO_EV_RCLCPP_PUBLISH`, `ARMO_EV_RCLCPP_INTRA_PUBLISH`, `ARMO_EV_RCL_PUBLISH`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_publish {
    pub publisher: u64,
    pub message: u64,
    pub stamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rmw_publish {
    pub rmw_publisher: u64,
    pub message: u64,
    pub timestamp: i64,
    pub stamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rmw_publish_rcl {
    pub message: u64,
    pub stamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_subscription_init {
    pub subscription_handle: u64,
    pub subscription: u64,
}

/// `ARMO_EV_RCLCPP_{SUBSCRIPTION,SERVICE,TIMER}_CALLBACK_ADDED`: the entity's handle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_callback_added {
    pub handle: u64,
    pub callback: u64,
}

/// `stamp` is read only when `taken` is 1.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rmw_take {
    pub rmw_subscription: u64,
    pub message: u64,
    pub source_timestamp: i64,
    pub stamp: u64,
    pub taken: u32,
    pub pad: u32,
}

/// `ARMO_EV_RCL_TAKE`, `ARMO_EV_RCLCPP_TAKE`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_take {
    pub message: u64,
}

/// Strings: service name. Also `ARMO_EV_RCL_CLIENT_INIT`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_service_init {
    pub handle: u64,
    pub node: u64,
    pub rmw_handle: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_timer_init {
    pub timer: u64,
    pub period: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_timer_link_node {
    pub timer: u64,
    pub node: u64,
}

/// Strings: the function symbol.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_callback_register {
    pub callback: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_callback_start {
    pub callback: u64,
    pub is_intra_process: u32,
    pub pad: u32,
}

/// `ARMO_EV_CALLBACK_END`, `ARMO_EV_RCL_TIMER_FINI` (the timer), `ARMO_EV_RCLCPP_EXECUTOR_EXECUTE`
/// (the executable's handle).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_handle {
    pub handle: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_executor_wait_for_work {
    pub timeout: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_wait_enter {
    pub wait_set: u64,
    pub timeout: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rcl_wait_exit {
    pub wait_set: u64,
    pub ret: i32,
    pub pad: u32,
}

/// An rclcpp constructor or method: `object` is its `this`, `arg` its object argument (a
/// `shared_ptr`'s first word) or id, 0 when it has none (design 2.1.2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_object {
    pub object: u64,
    pub arg: u64,
}

pub const ARMO_OWNER_SUBSCRIPTION: u32 = 1;
pub const ARMO_OWNER_SERVICE: u32 = 2;
pub const ARMO_OWNER_TIMER: u32 = 3;

/// `object` is what the matching `*_callback_added` event names: the `SubscriptionBase` for a
/// subscription, the rcl handle for a service or timer. `node` is the `rcl_node_t`, 0 when
/// unknown; the record's string is its fully qualified name.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_callback_owner {
    pub callback: u64,
    pub object: u64,
    pub handle: u64,
    pub node: u64,
    pub kind: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rclcpp_ipm_insert_sub_id_for_pub {
    pub ipm: u64,
    pub sub_id: u64,
    pub pub_id: u64,
    pub use_take_shared_method: u32,
    pub pad: u32,
}

/// The words at the callback handle's offsets 24, 16, 32, 40 and 8, in that order (design 2.4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_callback_code {
    pub callback: u64,
    pub words: [u64; 5],
}

/// Strings: topic, read from the rmw handle by a tool that attached late.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_rmw_topic {
    pub rmw_handle: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_dds_serialized_size {
    pub size: u64,
}

/// Strings: segment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_channel_init {
    pub channel: u64,
    pub dev: u64,
    pub ino: u64,
    pub publisher: i32,
    pub slot_count: u32,
    pub slot_size: u32,
    pub stamp_offset: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_claim {
    pub channel: u64,
    pub slot: u32,
    pub result: i32,
    pub loan: i32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_loan {
    pub channel: u64,
    pub slot: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_loan_end {
    pub channel: u64,
    pub slot: u32,
    pub result: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_commit {
    pub channel: u64,
    pub ticket: u64,
    pub nbytes: u64,
    pub stamp: u64,
    pub slot: u32,
    pub woke: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_take {
    pub channel: u64,
    pub ticket: u64,
    pub lost: u64,
    pub stamp: u64,
    pub slot: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_release {
    pub channel: u64,
    pub ticket: u64,
    pub held: i32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_refused {
    pub channel: u64,
    pub reason: i32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_wait {
    pub executor: u64,
    pub timeout_ns: i64,
    pub sources: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_wake {
    pub executor: u64,
    pub result: i32,
    pub entry: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_callback_start {
    pub executor: u64,
    pub endpoint: u64,
    pub entry: u32,
    pub pad: u32,
}

/// Strings: segment.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_callback_end {
    pub executor: u64,
    pub channel: u64,
    pub entry: u32,
    pub delivered: i32,
}

/// Strings: node, key, signpost.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_endpoint_init {
    pub endpoint: u64,
    pub fingerprint: u64,
    pub role: i32,
    pub lang: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_py_gil {
    pub phase: i32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_py_ready {
    pub channel: u64,
    pub ticket: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_record_drop {
    pub channel: u64,
    pub ticket: u64,
    pub count: u32,
    pub reason: i32,
}

/// `result` is the `CUresult` that wedged the GPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_flux_gpu_wedged {
    pub result: i32,
    pub pad: u32,
}

/// `writer` is the DDS writer handle or object the call took.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_dds_write {
    pub writer: u64,
    pub duration_ns: u64,
}

/// `bytes` is the copy size of a memcpy and the size of an alloc, else 0. `result` is the
/// `CUresult`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_cuda_call {
    pub duration_ns: u64,
    pub bytes: u64,
    pub api: u32,
    pub result: i32,
}

/// `key` is the address of the span's name, the same at its begin and end. Strings: name, on
/// `ARMO_EV_SPAN_BEGIN` only.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_span {
    pub key: u64,
}

/// `op` is the futex command without its flags; `ret` the system call's return value.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_futex_wait {
    pub uaddr: u64,
    pub duration_ns: u64,
    pub op: u32,
    pub ret: i32,
}

/// The record's position is the thread leaving the CPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_sched_switch {
    pub prev_state: i64,
    pub next_tid: u32,
    pub cpu: u32,
}

/// The record's position is the waker; `tid` is the woken thread.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct armo_ev_sched_wakeup {
    pub tid: u32,
    pub target_cpu: u32,
}

const _: () = assert!(size_of::<armo_record>() == 16);
const _: () = assert!(ARMO_EVFILE_MAGIC == u64::from_le_bytes(*b"ARMOEV1\0"));
