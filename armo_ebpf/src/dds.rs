//! Serialized message sizes inside the DDS implementations (design 2.1.2, bandwidth opt-in).
//! These are internal functions reached through function tables, so only a uprobe sees them.

use armo_common::{bpf::*, events::*};
use aya_ebpf::{
    helpers::bpf_probe_read_user,
    macros::{uprobe, uretprobe},
    programs::{ProbeContext, RetProbeContext},
};

use crate::{emit::*, maps::*};

/// `ddsi_serdata_kind` of a whole sample; a key-only serdata is not a message.
const SDK_DATA: u64 = 2;

fn size(size: u64) {
    let ns = now();
    if size != 0 && streamed(tgid()) {
        emit(
            ARMO_EV_DDS_SERIALIZED_SIZE,
            ns,
            &armo_ev_dds_serialized_size { size },
            [],
        );
    }
}

fn take_stash() -> Option<u64> {
    let t = tid();
    // SAFETY: the entry is this thread's own.
    let v = unsafe { *STASH.get_ptr(t)? };
    let _ = STASH.remove(t);
    Some(v)
}

// rmw_fastrtps_shared_cpp::TypeSupport::serialize(this, data, SerializedPayload_t * payload)
#[uprobe]
pub fn fastdds_serialize(ctx: ProbeContext) -> u32 {
    let payload = ctx.arg::<u64>(2).unwrap_or(0);
    if payload != 0 {
        let _ = STASH.insert(tid(), payload, 0);
    }
    0
}

/// `SerializedPayload_t::length` is the u32 at offset 4.
#[uretprobe]
pub fn fastdds_serialize_ret(ctx: RetProbeContext) -> u32 {
    if !mine(&ctx) {
        return 0;
    }
    let Some(payload) = take_stash() else {
        return 0;
    };
    if ctx.ret::<u64>() & 0xff == 0 {
        return 0;
    }
    // SAFETY: the kernel checks the user address.
    let len = unsafe { bpf_probe_read_user((payload + 4) as *const u32) }.unwrap_or(0);
    size(len as u64);
    0
}

// static ddsi_serdata * serdata_rmw_from_sample(const ddsi_sertype *, ddsi_serdata_kind, sample)
#[uprobe]
pub fn cyclone_from_sample(ctx: ProbeContext) -> u32 {
    let _ = STASH.insert(tid(), ctx.arg::<u64>(1).unwrap_or(0) & 0xffff_ffff, 0);
    0
}

/// `serdata_rmw::m_size` follows the `ddsi_serdata` base: offset 72, or 56 in a build without
/// shared memory. Both are sampled for the selfcheck, which sets `CONFIG_SERDATA_SIZE_OFFSET`;
/// until then 72 (design 6.7).
#[uretprobe]
pub fn cyclone_from_sample_ret(ctx: RetProbeContext) -> u32 {
    if !mine(&ctx) {
        return 0;
    }
    let Some(kind) = take_stash() else {
        return 0;
    };
    let serdata = ctx.ret::<u64>();
    if kind != SDK_DATA || serdata == 0 {
        return 0;
    }
    for (slot, offset) in [(SAMPLE_SERDATA_72, 72), (SAMPLE_SERDATA_56, 56)] {
        let v = read_u64(serdata + offset);
        if v != 0
            && v <= SERDATA_PLAUSIBLE_MAX
            && let Some(p) = SAMPLE.get_ptr_mut(slot)
        {
            add(p, 1);
        }
    }
    if let Some(p) = SAMPLE.get_ptr_mut(SAMPLE_SERDATA_N) {
        add(p, 1);
    }
    let offset = match config(CONFIG_SERDATA_SIZE_OFFSET) {
        0 => 72,
        o => o,
    };
    let v = read_u64(serdata + offset);
    if v <= SERDATA_PLAUSIBLE_MAX {
        size(v);
    }
    0
}
