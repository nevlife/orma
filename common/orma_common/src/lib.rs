//! orma's contracts: the eBPF map types ormad reads (here and `bpf`), the event records every collection method
//! writes (`events`, `catalog`) and the LD_PRELOAD shared memory layout (`shm`). C reads the last
//! two through headers cbindgen writes from those files.
#![no_std]

pub mod bpf;
pub mod catalog;
pub mod events;
pub mod shm;

/// The uprobe anchors of the rcl_executor core (`rcl_executor/trace.h`). A process whose mapped
/// librcl_executor.so exports all of them gets the rcl_executor probes.
pub const RCL_EXECUTOR_ANCHORS: [&str; 4] = [
    "rcl_executor_trace_wait_enter",
    "rcl_executor_trace_wait_exit",
    "rcl_executor_trace_item_claimed",
    "rcl_executor_trace_item_completed",
];

/// The broker anchors of the overhaul's rcl (`rcl/intra_process.h`) and the symbol that names
/// their arguments. A librcl.so that exports the contract gets the broker probes.
pub const RCL_IPC_CONTRACT: &str = "rcl_ipc_trace_contract_1";
pub const RCL_IPC_ANCHORS: [&str; 3] = [
    "rcl_ipc_trace_publish",
    "rcl_ipc_trace_take",
    "rcl_ipc_trace_release",
];

/// The conversion anchors of the overhaul's rclpy, exported by its `_rclpy_pybind11` module.
pub const RCLPY_CONTRACT: &str = "rclpy_trace_contract_1";
pub const RCLPY_ANCHORS: [&str; 2] = ["rclpy_trace_take_converted", "rclpy_trace_publish_enter"];

/// 2^40 ns is about 18 minutes.
pub const BUCKETS: usize = shm::ORMA_BUCKETS;
pub const NAME_LEN: usize = 128;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CallbackStart {
    pub callback: u64,
    pub ns: u64,
}

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
        orma_shm_header,
        orma_thread_slot,
        orma_table_entry,
        orma_record,
        orma_position,
        orma_evfile_header,
        orma_ev_rcl_init,
        orma_ev_rcl_node_init,
        orma_ev_rmw_endpoint_init,
        orma_ev_rcl_topic_init,
        orma_ev_publish,
        orma_ev_rmw_publish,
        orma_ev_rmw_publish_rcl,
        orma_ev_rclcpp_subscription_init,
        orma_ev_callback_added,
        orma_ev_rmw_take,
        orma_ev_take,
        orma_ev_rcl_service_init,
        orma_ev_rcl_timer_init,
        orma_ev_rclcpp_timer_link_node,
        orma_ev_rclcpp_callback_register,
        orma_ev_callback_start,
        orma_ev_handle,
        orma_ev_rclcpp_executor_wait_for_work,
        orma_ev_rcl_wait_enter,
        orma_ev_rcl_wait_exit,
        orma_ev_rclcpp_object,
        orma_ev_rclcpp_callback_owner,
        orma_ev_rclcpp_ipm_insert_sub_id_for_pub,
        orma_ev_rcl_executor_claim,
        orma_ev_rcl_executor_complete,
        orma_ev_rcl_ipc_publish,
        orma_ev_rcl_ipc_take,
        orma_ev_rcl_loan,
        orma_ev_rclpy_take,
        orma_ev_callback_code,
        orma_ev_rmw_topic,
        orma_ev_dds_serialized_size,
        orma_ev_flux_channel_init,
        orma_ev_flux_claim,
        orma_ev_flux_loan,
        orma_ev_flux_loan_end,
        orma_ev_flux_commit,
        orma_ev_flux_take,
        orma_ev_flux_release,
        orma_ev_flux_refused,
        orma_ev_flux_wait,
        orma_ev_flux_wake,
        orma_ev_flux_callback_start,
        orma_ev_flux_callback_end,
        orma_ev_flux_endpoint_init,
        orma_ev_flux_py_gil,
        orma_ev_flux_py_ready,
        orma_ev_flux_record_drop,
        orma_ev_flux_gpu_wedged,
        orma_ev_sched_switch,
        orma_ev_sched_wakeup,
        orma_ev_dds_write,
        orma_ev_cuda_call,
        orma_ev_span,
        orma_ev_futex_wait,
    );
}

#[cfg(feature = "user")]
#[allow(unsafe_code)]
mod pod {
    use super::*;
    // SAFETY: repr(C) integers shared with the eBPF programs; any bit pattern is a value.
    unsafe impl aya::Pod for CallbackStart {}
}
