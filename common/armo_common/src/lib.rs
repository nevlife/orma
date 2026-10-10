//! armo's contracts: the eBPF map types armod reads (here and `bpf`), the event records every collection method
//! writes (`events`, `catalog`) and the LD_PRELOAD shared memory layout (`shm`). C reads the last
//! two through headers cbindgen writes from those files.
#![no_std]

pub mod bpf;
pub mod catalog;
pub mod events;
pub mod shm;

/// 2^40 ns is about 18 minutes.
pub const BUCKETS: usize = shm::ARMO_BUCKETS;
pub const NAME_LEN: usize = 128;

pub const KIND_ROS: u32 = 0;
pub const KIND_FLUX: u32 = 1;
/// A `NAMES` key only: `callback` is a flux channel handle.
pub const KIND_FLUX_CHANNEL: u32 = 2;
/// Keyed like `KIND_FLUX`; its `NAMES` entry is the segment the callback first delivered from.
pub const KIND_FLUX2: u32 = 3;

/// One callback in one process. For `KIND_ROS`, `callback` is the address rclcpp passes to
/// `ros_trace_*` and `entry` is 0. For `KIND_FLUX`, `callback` is the executor and `entry` its
/// index there: unlike the channel handle, it survives a publisher restart.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CallbackKey {
    pub tgid: u32,
    pub kind: u32,
    pub callback: u64,
    pub entry: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackStart {
    pub callback: u64,
    pub ns: u64,
}

/// Keyed by executor as well as thread: a callback can dispatch another executor inside it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FluxStartKey {
    pub tid: u64,
    pub executor: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FluxStart {
    pub ns: u64,
    pub entry: u32,
    pub pad: u32,
}

/// Cumulative since the entry was created. Updated with atomic adds only, so concurrent callbacks
/// on other CPUs do not lose counts.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackStat {
    pub count: u64,
    pub sum_ns: u64,
    pub hist: [u64; BUCKETS],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackName {
    pub bytes: [u8; NAME_LEN],
}

/// Indexes into the `LOST` array: events the programs could not record.
pub const LOST_STAT_FULL: u32 = 0;
pub const LOST_NAME_FULL: u32 = 1;
pub const LOST_SLOTS: u32 = 2;

pub const fn bucket(ns: u64) -> usize {
    let mut v = ns;
    let mut b = 0usize;
    if v >= 1 << 32 {
        v >>= 32;
        b += 32;
    }
    if v >= 1 << 16 {
        v >>= 16;
        b += 16;
    }
    if v >= 1 << 8 {
        v >>= 8;
        b += 8;
    }
    if v >= 1 << 4 {
        v >>= 4;
        b += 4;
    }
    if v >= 1 << 2 {
        v >>= 2;
        b += 2;
    }
    if v >= 1 << 1 {
        b += 1;
    }
    if b >= BUCKETS { BUCKETS - 1 } else { b }
}

/// A `#[repr(C)]` type of integers with no padding, so every byte pattern is a value and every
/// byte of a value is initialized. The layout test checks that the fields of each tile its size.
///
/// # Safety
/// Implement only for such types.
#[allow(unsafe_code)]
pub unsafe trait Plain: Copy {}

pub fn bytes_of<T: Plain>(value: &T) -> &[u8] {
    #[allow(unsafe_code)]
    // SAFETY: Plain types have no padding, so all size_of::<T>() bytes are initialized.
    unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>())
    }
}

/// The value at the start of `bytes`, which need not be aligned.
pub fn read<T: Plain>(bytes: &[u8]) -> Option<T> {
    if bytes.len() < size_of::<T>() {
        return None;
    }
    #[allow(unsafe_code)]
    // SAFETY: the length is checked, and any bytes are a valid Plain value.
    Some(unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) })
}

#[allow(unsafe_code)]
mod plain {
    use super::Plain;
    use super::events::*;
    use super::shm::*;

    macro_rules! plain {
        ($($t:ty),* $(,)?) => {
            // SAFETY: generated repr(C) integer records; tests/layout.rs checks they tile.
            $(unsafe impl Plain for $t {})*
        };
    }

    plain!(
        (),
        armo_shm_header,
        armo_thread_slot,
        armo_table_entry,
        armo_record,
        armo_position,
        armo_evfile_header,
        armo_ev_rcl_init,
        armo_ev_rcl_node_init,
        armo_ev_rmw_endpoint_init,
        armo_ev_rcl_topic_init,
        armo_ev_publish,
        armo_ev_rmw_publish,
        armo_ev_rmw_publish_rcl,
        armo_ev_rclcpp_subscription_init,
        armo_ev_callback_added,
        armo_ev_rmw_take,
        armo_ev_take,
        armo_ev_rcl_service_init,
        armo_ev_rcl_timer_init,
        armo_ev_rclcpp_timer_link_node,
        armo_ev_rclcpp_callback_register,
        armo_ev_callback_start,
        armo_ev_handle,
        armo_ev_rclcpp_executor_wait_for_work,
        armo_ev_rmw_send_request,
        armo_ev_rmw_take_request,
        armo_ev_rmw_send_response,
        armo_ev_rmw_take_response,
        armo_ev_rcl_wait_enter,
        armo_ev_rcl_wait_exit,
        armo_ev_rclcpp_object,
        armo_ev_rclcpp_callback_owner,
        armo_ev_rclcpp_ipm_insert_sub_id_for_pub,
        armo_ev_callback_code,
        armo_ev_rmw_topic,
        armo_ev_dds_serialized_size,
        armo_ev_flux2_channel_init,
        armo_ev_flux2_claim,
        armo_ev_flux2_loan,
        armo_ev_flux2_loan_end,
        armo_ev_flux2_commit,
        armo_ev_flux2_take,
        armo_ev_flux2_release,
        armo_ev_flux2_refused,
        armo_ev_flux2_wait,
        armo_ev_flux2_wake,
        armo_ev_flux2_callback_start,
        armo_ev_flux2_callback_end,
        armo_ev_flux2_endpoint_init,
        armo_ev_flux2_py_gil,
        armo_ev_flux2_py_ready,
        armo_ev_flux2_record_drop,
        armo_ev_flux2_gpu_wedged,
        armo_ev_sched_switch,
        armo_ev_sched_wakeup,
    );
}

#[cfg(feature = "user")]
#[allow(unsafe_code)]
mod pod {
    use super::*;
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for CallbackKey {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for CallbackStart {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for FluxStart {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for FluxStartKey {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for CallbackStat {}
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for CallbackName {}
}
