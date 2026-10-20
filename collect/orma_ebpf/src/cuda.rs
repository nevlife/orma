//! CUDA driver API calls in libcuda.so (design 20.1). Each group of functions has an entry
//! program that knows its `api` and where its byte count is; `cuda_ret`, attached to every one,
//! writes the record when the call returns.

use aya_ebpf::{
    macros::{uprobe, uretprobe},
    programs::{ProbeContext, RetProbeContext},
};
use orma_common::{bpf::LOST_UNPAIRED, events::*};

use crate::{emit::*, maps::*};

/// `WidthInBytes` and `Height` of `CUDA_MEMCPY2D`, and `Depth` of `CUDA_MEMCPY3D` (cuda.h of
/// CUDA 13.2; unchanged since the `_v2` structs of CUDA 3.2).
const MEMCPY2D_WIDTH: u64 = 112;
const MEMCPY3D_WIDTH: u64 = 176;

fn arg(ctx: &ProbeContext, n: usize) -> u64 {
    ctx.arg::<u64>(n).unwrap_or(0)
}

fn enter(api: u32, bytes: u64) -> u32 {
    if streamed(tgid()) {
        let p = Pending {
            ns: now(),
            arg: bytes,
            kind: api,
            pad: 0,
        };
        let _ = CUDA_CALLS.insert(tid(), p, 0);
    }
    0
}

fn area_2d(copy: u64) -> u64 {
    read_u64(copy + MEMCPY2D_WIDTH).wrapping_mul(read_u64(copy + MEMCPY2D_WIDTH + 8))
}

fn volume_3d(copy: u64) -> u64 {
    let [w, h, d] = [0, 8, 16].map(|o| read_u64(copy + MEMCPY3D_WIDTH + o));
    w.wrapping_mul(h).wrapping_mul(d)
}

#[uprobe]
pub fn cuda_launch(_ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_LAUNCH, 0)
}

#[uprobe]
pub fn cuda_graph_launch(_ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_GRAPH_LAUNCH, 0)
}

// (dst, src, ByteCount): cuMemcpy, cuMemcpyHtoD_v2, cuMemcpyDtoH_v2, cuMemcpyDtoD_v2
#[uprobe]
pub fn cuda_memcpy(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY, arg(&ctx, 2))
}

// The same with a stream after ByteCount.
#[uprobe]
pub fn cuda_memcpy_async(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY_ASYNC, arg(&ctx, 2))
}

// (dstDevice, dstContext, srcDevice, srcContext, ByteCount[, hStream])
#[uprobe]
pub fn cuda_memcpy_peer(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY, arg(&ctx, 4))
}

#[uprobe]
pub fn cuda_memcpy_peer_async(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY_ASYNC, arg(&ctx, 4))
}

// (const CUDA_MEMCPY2D *pCopy[, hStream])
#[uprobe]
pub fn cuda_memcpy_2d(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY, area_2d(arg(&ctx, 0)))
}

#[uprobe]
pub fn cuda_memcpy_2d_async(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY_ASYNC, area_2d(arg(&ctx, 0)))
}

// (const CUDA_MEMCPY3D *pCopy[, hStream])
#[uprobe]
pub fn cuda_memcpy_3d(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY, volume_3d(arg(&ctx, 0)))
}

#[uprobe]
pub fn cuda_memcpy_3d_async(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_MEMCPY_ASYNC, volume_3d(arg(&ctx, 0)))
}

#[uprobe]
pub fn cuda_sync_stream(_ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_SYNC_STREAM, 0)
}

#[uprobe]
pub fn cuda_sync_ctx(_ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_SYNC_CTX, 0)
}

#[uprobe]
pub fn cuda_sync_event(_ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_SYNC_EVENT, 0)
}

// (pointer out, bytesize, ...): cuMemAlloc_v2, cuMemAllocAsync, cuMemAllocManaged,
// cuMemHostAlloc, cuMemAllocHost_v2
#[uprobe]
pub fn cuda_alloc(ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_ALLOC, arg(&ctx, 1))
}

#[uprobe]
pub fn cuda_free(_ctx: ProbeContext) -> u32 {
    enter(ORMA_CUDA_FREE, 0)
}

#[uretprobe]
pub fn cuda_ret(ctx: RetProbeContext) -> u32 {
    let ns = now();
    if !mine(&ctx) || !streamed(tgid()) {
        return 0;
    }
    let Some(p) = take(&CUDA_CALLS) else {
        // The LRU map evicted it, or the call began before the attach or the stream.
        lost(LOST_UNPAIRED);
        return 0;
    };
    let ev = orma_ev_cuda_call {
        duration_ns: ns.saturating_sub(p.ns),
        bytes: p.arg,
        api: p.kind,
        result: ctx.ret::<i32>(),
    };
    emit(ORMA_EV_CUDA_CALL, ns, &ev, []);
    0
}
