// The CUDA driver functions orma times (design 20.1), with cuda.h's prototypes but CUresult as int
// and handles opaque, so that orma builds without the CUDA toolkit. One X per exported variant:
// X(name, api, bytes, params, args). _ptds and _ptsz take the per-thread default stream; a
// parameter name has one type throughout, so that a test can call every entry.
#ifndef ORMA_PRELOAD_CUDA_API_H
#define ORMA_PRELOAD_CUDA_API_H

#include <stddef.h>
#include <stdint.h>

typedef int CUresult;
typedef unsigned long long CUdeviceptr;
typedef struct CUctx_st * CUcontext;
typedef struct CUstream_st * CUstream;
typedef struct CUevent_st * CUevent;
typedef struct CUfunc_st * CUfunction;
typedef struct CUgraphExec_st * CUgraphExec;
typedef struct CUlaunchConfig_st CUlaunchConfig;

// CUDA_MEMCPY2D_v2 and CUDA_MEMCPY3D_v2.
typedef struct orma_cuda_memcpy2d
{
  size_t src_x, src_y;
  int src_type;
  const void * src_host;
  CUdeviceptr src_device;
  void * src_array;
  size_t src_pitch, dst_x, dst_y;
  int dst_type;
  void * dst_host;
  CUdeviceptr dst_device;
  void * dst_array;
  size_t dst_pitch, width, height;
} orma_cuda_memcpy2d;

typedef struct orma_cuda_memcpy3d
{
  size_t src_x, src_y, src_z, src_lod;
  int src_type;
  const void * src_host;
  CUdeviceptr src_device;
  void * src_array;
  void * reserved0;
  size_t src_pitch, src_height, dst_x, dst_y, dst_z, dst_lod;
  int dst_type;
  void * dst_host;
  CUdeviceptr dst_device;
  void * dst_array;
  void * reserved1;
  size_t dst_pitch, dst_height, width, height, depth;
} orma_cuda_memcpy3d;

_Static_assert(sizeof(void *) == 8, "CUDA's 64-bit layouts");
_Static_assert(offsetof(orma_cuda_memcpy2d, width) == 112, "CUDA_MEMCPY2D");
_Static_assert(offsetof(orma_cuda_memcpy3d, width) == 176, "CUDA_MEMCPY3D");

#define CUDA_ERROR_NOT_FOUND 500
// CU_GET_PROC_ADDRESS_PER_THREAD_DEFAULT_STREAM
#define ORMA_CU_PER_THREAD 2

#define ORMA_CU_COPY (CUdeviceptr ddst, CUdeviceptr dsrc, size_t count)
#define ORMA_CU_COPY_A (ddst, dsrc, count)
#define ORMA_CU_HTOD (CUdeviceptr ddst, const void * hsrc, size_t count)
#define ORMA_CU_HTOD_A (ddst, hsrc, count)
#define ORMA_CU_DTOH (void * hdst, CUdeviceptr dsrc, size_t count)
#define ORMA_CU_DTOH_A (hdst, dsrc, count)
#define ORMA_CU_PEER \
  (CUdeviceptr ddst, CUcontext dctx, CUdeviceptr dsrc, CUcontext sctx, size_t count)
#define ORMA_CU_PEER_A (ddst, dctx, dsrc, sctx, count)
#define ORMA_CU_2D (const orma_cuda_memcpy2d * copy2d)
#define ORMA_CU_3D (const orma_cuda_memcpy3d * copy3d)

#define ORMA_CU_COPY_S (CUdeviceptr ddst, CUdeviceptr dsrc, size_t count, CUstream stream)
#define ORMA_CU_COPY_SA (ddst, dsrc, count, stream)
#define ORMA_CU_HTOD_S (CUdeviceptr ddst, const void * hsrc, size_t count, CUstream stream)
#define ORMA_CU_HTOD_SA (ddst, hsrc, count, stream)
#define ORMA_CU_DTOH_S (void * hdst, CUdeviceptr dsrc, size_t count, CUstream stream)
#define ORMA_CU_DTOH_SA (hdst, dsrc, count, stream)
#define ORMA_CU_PEER_S                                                               \
  (CUdeviceptr ddst, CUcontext dctx, CUdeviceptr dsrc, CUcontext sctx, size_t count, \
   CUstream stream)
#define ORMA_CU_PEER_SA (ddst, dctx, dsrc, sctx, count, stream)
#define ORMA_CU_2D_S (const orma_cuda_memcpy2d * copy2d, CUstream stream)
#define ORMA_CU_3D_S (const orma_cuda_memcpy3d * copy3d, CUstream stream)

#define ORMA_CU_LAUNCH                                                                      \
  (CUfunction f, unsigned int gx, unsigned int gy, unsigned int gz, unsigned int bx,        \
   unsigned int by, unsigned int bz, unsigned int shared, CUstream stream, void ** kparams, \
   void ** extra)
#define ORMA_CU_LAUNCH_A (f, gx, gy, gz, bx, by, bz, shared, stream, kparams, extra)
#define ORMA_CU_LAUNCH_EX \
  (const CUlaunchConfig * config, CUfunction f, void ** kparams, void ** extra)
#define ORMA_CU_LAUNCH_EX_A (config, f, kparams, extra)
#define ORMA_CU_COOP                                                                 \
  (CUfunction f, unsigned int gx, unsigned int gy, unsigned int gz, unsigned int bx, \
   unsigned int by, unsigned int bz, unsigned int shared, CUstream stream, void ** kparams)
#define ORMA_CU_COOP_A (f, gx, gy, gz, bx, by, bz, shared, stream, kparams)

#define ORMA_CU_2D_BYTES ((uint64_t)copy2d->width * copy2d->height)
#define ORMA_CU_3D_BYTES ((uint64_t)copy3d->width * copy3d->height * copy3d->depth)

#define ORMA_CUDA_FUNCTIONS(X)                                                                   \
  X(cuLaunchKernel, LAUNCH, 0, ORMA_CU_LAUNCH, ORMA_CU_LAUNCH_A)                                 \
  X(cuLaunchKernel_ptsz, LAUNCH, 0, ORMA_CU_LAUNCH, ORMA_CU_LAUNCH_A)                            \
  X(cuLaunchKernelEx, LAUNCH, 0, ORMA_CU_LAUNCH_EX, ORMA_CU_LAUNCH_EX_A)                         \
  X(cuLaunchKernelEx_ptsz, LAUNCH, 0, ORMA_CU_LAUNCH_EX, ORMA_CU_LAUNCH_EX_A)                    \
  X(cuLaunchCooperativeKernel, LAUNCH, 0, ORMA_CU_COOP, ORMA_CU_COOP_A)                          \
  X(cuLaunchCooperativeKernel_ptsz, LAUNCH, 0, ORMA_CU_COOP, ORMA_CU_COOP_A)                     \
  X(cuGraphLaunch, GRAPH_LAUNCH, 0, (CUgraphExec graph, CUstream stream), (graph, stream))       \
  X(cuGraphLaunch_ptsz, GRAPH_LAUNCH, 0, (CUgraphExec graph, CUstream stream), (graph, stream))  \
  X(cuMemcpy, MEMCPY, count, ORMA_CU_COPY, ORMA_CU_COPY_A)                                       \
  X(cuMemcpy_ptds, MEMCPY, count, ORMA_CU_COPY, ORMA_CU_COPY_A)                                  \
  X(cuMemcpyHtoD_v2, MEMCPY, count, ORMA_CU_HTOD, ORMA_CU_HTOD_A)                                \
  X(cuMemcpyHtoD_v2_ptds, MEMCPY, count, ORMA_CU_HTOD, ORMA_CU_HTOD_A)                           \
  X(cuMemcpyDtoH_v2, MEMCPY, count, ORMA_CU_DTOH, ORMA_CU_DTOH_A)                                \
  X(cuMemcpyDtoH_v2_ptds, MEMCPY, count, ORMA_CU_DTOH, ORMA_CU_DTOH_A)                           \
  X(cuMemcpyDtoD_v2, MEMCPY, count, ORMA_CU_COPY, ORMA_CU_COPY_A)                                \
  X(cuMemcpyDtoD_v2_ptds, MEMCPY, count, ORMA_CU_COPY, ORMA_CU_COPY_A)                           \
  X(cuMemcpy2D_v2, MEMCPY, ORMA_CU_2D_BYTES, ORMA_CU_2D, (copy2d))                               \
  X(cuMemcpy2D_v2_ptds, MEMCPY, ORMA_CU_2D_BYTES, ORMA_CU_2D, (copy2d))                          \
  X(cuMemcpy2DUnaligned_v2, MEMCPY, ORMA_CU_2D_BYTES, ORMA_CU_2D, (copy2d))                      \
  X(cuMemcpy2DUnaligned_v2_ptds, MEMCPY, ORMA_CU_2D_BYTES, ORMA_CU_2D, (copy2d))                 \
  X(cuMemcpy3D_v2, MEMCPY, ORMA_CU_3D_BYTES, ORMA_CU_3D, (copy3d))                               \
  X(cuMemcpy3D_v2_ptds, MEMCPY, ORMA_CU_3D_BYTES, ORMA_CU_3D, (copy3d))                          \
  X(cuMemcpyPeer, MEMCPY, count, ORMA_CU_PEER, ORMA_CU_PEER_A)                                   \
  X(cuMemcpyPeer_ptds, MEMCPY, count, ORMA_CU_PEER, ORMA_CU_PEER_A)                              \
  X(cuMemcpyAsync, MEMCPY_ASYNC, count, ORMA_CU_COPY_S, ORMA_CU_COPY_SA)                         \
  X(cuMemcpyAsync_ptsz, MEMCPY_ASYNC, count, ORMA_CU_COPY_S, ORMA_CU_COPY_SA)                    \
  X(cuMemcpyHtoDAsync_v2, MEMCPY_ASYNC, count, ORMA_CU_HTOD_S, ORMA_CU_HTOD_SA)                  \
  X(cuMemcpyHtoDAsync_v2_ptsz, MEMCPY_ASYNC, count, ORMA_CU_HTOD_S, ORMA_CU_HTOD_SA)             \
  X(cuMemcpyDtoHAsync_v2, MEMCPY_ASYNC, count, ORMA_CU_DTOH_S, ORMA_CU_DTOH_SA)                  \
  X(cuMemcpyDtoHAsync_v2_ptsz, MEMCPY_ASYNC, count, ORMA_CU_DTOH_S, ORMA_CU_DTOH_SA)             \
  X(cuMemcpyDtoDAsync_v2, MEMCPY_ASYNC, count, ORMA_CU_COPY_S, ORMA_CU_COPY_SA)                  \
  X(cuMemcpyDtoDAsync_v2_ptsz, MEMCPY_ASYNC, count, ORMA_CU_COPY_S, ORMA_CU_COPY_SA)             \
  X(cuMemcpy2DAsync_v2, MEMCPY_ASYNC, ORMA_CU_2D_BYTES, ORMA_CU_2D_S, (copy2d, stream))          \
  X(cuMemcpy2DAsync_v2_ptsz, MEMCPY_ASYNC, ORMA_CU_2D_BYTES, ORMA_CU_2D_S, (copy2d, stream))     \
  X(cuMemcpy3DAsync_v2, MEMCPY_ASYNC, ORMA_CU_3D_BYTES, ORMA_CU_3D_S, (copy3d, stream))          \
  X(cuMemcpy3DAsync_v2_ptsz, MEMCPY_ASYNC, ORMA_CU_3D_BYTES, ORMA_CU_3D_S, (copy3d, stream))     \
  X(cuMemcpyPeerAsync, MEMCPY_ASYNC, count, ORMA_CU_PEER_S, ORMA_CU_PEER_SA)                     \
  X(cuMemcpyPeerAsync_ptsz, MEMCPY_ASYNC, count, ORMA_CU_PEER_S, ORMA_CU_PEER_SA)                \
  X(cuStreamSynchronize, SYNC_STREAM, 0, (CUstream stream), (stream))                            \
  X(cuStreamSynchronize_ptsz, SYNC_STREAM, 0, (CUstream stream), (stream))                       \
  X(cuCtxSynchronize, SYNC_CTX, 0, (void), ())                                                   \
  X(cuCtxSynchronize_v2, SYNC_CTX, 0, (CUcontext ctx), (ctx))                                    \
  X(cuEventSynchronize, SYNC_EVENT, 0, (CUevent event), (event))                                 \
  X(cuMemAlloc_v2, ALLOC, count, (CUdeviceptr * dptr_out, size_t count), (dptr_out, count))      \
  X(cuMemAllocAsync, ALLOC, count, (CUdeviceptr * dptr_out, size_t count, CUstream stream),      \
    (dptr_out, count, stream))                                                                   \
  X(cuMemAllocAsync_ptsz, ALLOC, count, (CUdeviceptr * dptr_out, size_t count, CUstream stream), \
    (dptr_out, count, stream))                                                                   \
  X(cuMemAllocManaged, ALLOC, count, (CUdeviceptr * dptr_out, size_t count, unsigned int flags), \
    (dptr_out, count, flags))                                                                    \
  X(cuMemHostAlloc, ALLOC, count, (void ** hptr_out, size_t count, unsigned int flags),          \
    (hptr_out, count, flags))                                                                    \
  X(cuMemAllocHost_v2, ALLOC, count, (void ** hptr_out, size_t count), (hptr_out, count))        \
  X(cuMemFree_v2, FREE, 0, (CUdeviceptr dptr), (dptr))                                           \
  X(cuMemFreeAsync, FREE, 0, (CUdeviceptr dptr, CUstream stream), (dptr, stream))                \
  X(cuMemFreeAsync_ptsz, FREE, 0, (CUdeviceptr dptr, CUstream stream), (dptr, stream))           \
  X(cuMemFreeHost, FREE, 0, (void * hptr), (hptr))

// cuGetProcAddress before CUDA 12 had no status argument; _v2 points it at a
// CUdriverProcAddressQueryResult.
typedef CUresult (*orma_cu_get_proc_address)(const char *, void **, int, uint64_t);
typedef CUresult (*orma_cu_get_proc_address_v2)(const char *, void **, int, uint64_t, int *);

#endif  // ORMA_PRELOAD_CUDA_API_H
