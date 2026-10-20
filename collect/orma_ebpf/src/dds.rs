//! Serialized message sizes inside the DDS implementations (design 2.1.2, bandwidth opt-in):
//! internal functions reached through function tables, so only a uprobe sees them. And the DDS
//! write calls under `rmw_publish` (design 20.2, dds opt-in).

use aya_ebpf::{
    helpers::bpf_probe_read_user,
    macros::{uprobe, uretprobe},
    programs::{ProbeContext, RetProbeContext},
};
use orma_common::{bpf::*, events::*};

use crate::{emit::*, maps::*};

/// `ddsi_serdata_kind` of a whole sample; a key-only serdata is not a message.
const SDK_DATA: u64 = 2;

fn size(size: u64) {
    let ns = now();
    if size != 0 && streamed(tgid()) {
        emit(
            ORMA_EV_DDS_SERIALIZED_SIZE,
            ns,
            &orma_ev_dds_serialized_size { size },
            [],
        );
    }
}

fn take_stash() -> Option<u64> {
    let t = tid();
    let Some(p) = STASH.get_ptr(t) else {
        // The LRU map evicted the entry, or the call began before the attach.
        lost(LOST_UNPAIRED);
        return None;
    };
    // SAFETY: the entry is this thread's own.
    let v = unsafe { *p };
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

fn write_enter(writer: u64) -> u32 {
    if streamed(tgid()) {
        let p = Pending {
            ns: now(),
            arg: writer,
            kind: 0,
            pad: 0,
        };
        let _ = DDS_CALLS.insert(tid(), p, 0);
    }
    0
}

fn write_ret(ctx: &RetProbeContext) -> u32 {
    let ns = now();
    if !mine(ctx) || !streamed(tgid()) {
        return 0;
    }
    let Some(p) = take(&DDS_CALLS) else {
        lost(LOST_UNPAIRED);
        return 0;
    };
    let ev = orma_ev_dds_write {
        writer: p.arg,
        duration_ns: ns.saturating_sub(p.ns),
    };
    emit(ORMA_EV_DDS_WRITE, ns, &ev, []);
    0
}

// dds_write(writer, data), dds_write_ts(writer, data, timestamp), dds_writecdr(writer, serdata),
// dds_forwardcdr(writer, serdata). `dds_entity_t` is an int32.
#[uprobe]
pub fn cyclone_write(ctx: ProbeContext) -> u32 {
    write_enter(ctx.arg::<u64>(0).unwrap_or(0) & 0xffff_ffff)
}

#[uretprobe]
pub fn cyclone_write_ret(ctx: RetProbeContext) -> u32 {
    write_ret(&ctx)
}

// eprosima::fastdds::dds::DataWriter::write(this, data, ...)
#[uprobe]
pub fn fastdds_write(ctx: ProbeContext) -> u32 {
    write_enter(ctx.arg::<u64>(0).unwrap_or(0))
}

#[uretprobe]
pub fn fastdds_write_ret(ctx: RetProbeContext) -> u32 {
    write_ret(&ctx)
}
