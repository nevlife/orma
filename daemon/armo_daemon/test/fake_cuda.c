// A stand-in libcuda.so.1 for accuracy.py's gpu case: the driver functions armod attaches to,
// with cuda.h's signatures, doing nothing. The synchronize calls sleep 2 ms; cuEventSynchronize
// and cuMemFreeHost return an error. Build with -O0: identical bodies folded into one address
// would put two programs on one function.
#include <stddef.h>
#include <time.h>

typedef int CUresult;
typedef unsigned long long CUdeviceptr;
typedef void * CUcontext;
typedef void * CUstream;
typedef void * CUevent;
typedef void * CUfunction;
typedef void * CUgraphExec;
typedef void CUlaunchConfig;

// WidthInBytes and Height at 112 and 120 as in cuda.h's CUDA_MEMCPY2D.
typedef struct
{
  size_t before[14];
  size_t WidthInBytes;
  size_t Height;
} CUDA_MEMCPY2D;

// WidthInBytes, Height and Depth at 176, 184 and 192 as in cuda.h's CUDA_MEMCPY3D.
typedef struct
{
  size_t before[22];
  size_t WidthInBytes;
  size_t Height;
  size_t Depth;
} CUDA_MEMCPY3D;

static CUresult sync_sleep(CUresult r)
{
  struct timespec t = {0, 2000000};
  nanosleep(&t, NULL);
  return r;
}

#define LAUNCH(name)                                                                            \
  CUresult name(                                                                                \
    CUfunction f, unsigned gx, unsigned gy, unsigned gz, unsigned bx, unsigned by, unsigned bz, \
    unsigned shared, CUstream s, void ** params, void ** extra)                                 \
  {                                                                                             \
    return 0;                                                                                   \
  }
LAUNCH(cuLaunchKernel)
LAUNCH(cuLaunchKernel_ptsz)

#define LAUNCH_EX(name) \
  CUresult name(const CUlaunchConfig * c, CUfunction f, void ** params, void ** extra) { return 0; }
LAUNCH_EX(cuLaunchKernelEx)
LAUNCH_EX(cuLaunchKernelEx_ptsz)

#define COOPERATIVE(name)                                                                       \
  CUresult name(                                                                                \
    CUfunction f, unsigned gx, unsigned gy, unsigned gz, unsigned bx, unsigned by, unsigned bz, \
    unsigned shared, CUstream s, void ** params)                                                \
  {                                                                                             \
    return 0;                                                                                   \
  }
COOPERATIVE(cuLaunchCooperativeKernel)
COOPERATIVE(cuLaunchCooperativeKernel_ptsz)

CUresult cuGraphLaunch(CUgraphExec g, CUstream s) { return 0; }
CUresult cuGraphLaunch_ptsz(CUgraphExec g, CUstream s) { return 0; }

#define MEMCPY(name, dst_t, src_t) \
  CUresult name(dst_t dst, src_t src, size_t n) { return 0; }
MEMCPY(cuMemcpy, CUdeviceptr, CUdeviceptr)
MEMCPY(cuMemcpy_ptds, CUdeviceptr, CUdeviceptr)
MEMCPY(cuMemcpyHtoD_v2, CUdeviceptr, const void *)
MEMCPY(cuMemcpyHtoD_v2_ptds, CUdeviceptr, const void *)
MEMCPY(cuMemcpyDtoH_v2, void *, CUdeviceptr)
MEMCPY(cuMemcpyDtoH_v2_ptds, void *, CUdeviceptr)
MEMCPY(cuMemcpyDtoD_v2, CUdeviceptr, CUdeviceptr)
MEMCPY(cuMemcpyDtoD_v2_ptds, CUdeviceptr, CUdeviceptr)

#define MEMCPY_ASYNC(name, dst_t, src_t) \
  CUresult name(dst_t dst, src_t src, size_t n, CUstream s) { return 0; }
MEMCPY_ASYNC(cuMemcpyAsync, CUdeviceptr, CUdeviceptr)
MEMCPY_ASYNC(cuMemcpyAsync_ptsz, CUdeviceptr, CUdeviceptr)
MEMCPY_ASYNC(cuMemcpyHtoDAsync_v2, CUdeviceptr, const void *)
MEMCPY_ASYNC(cuMemcpyHtoDAsync_v2_ptsz, CUdeviceptr, const void *)
MEMCPY_ASYNC(cuMemcpyDtoHAsync_v2, void *, CUdeviceptr)
MEMCPY_ASYNC(cuMemcpyDtoHAsync_v2_ptsz, void *, CUdeviceptr)
MEMCPY_ASYNC(cuMemcpyDtoDAsync_v2, CUdeviceptr, CUdeviceptr)
MEMCPY_ASYNC(cuMemcpyDtoDAsync_v2_ptsz, CUdeviceptr, CUdeviceptr)

#define PEER(name) \
  CUresult name(CUdeviceptr dst, CUcontext dc, CUdeviceptr src, CUcontext sc, size_t n) { return 0; }
PEER(cuMemcpyPeer)
PEER(cuMemcpyPeer_ptds)

#define PEER_ASYNC(name)                                                                  \
  CUresult name(                                                                          \
    CUdeviceptr dst, CUcontext dc, CUdeviceptr src, CUcontext sc, size_t n, CUstream s) \
  {                                                                                       \
    return 0;                                                                             \
  }
PEER_ASYNC(cuMemcpyPeerAsync)
PEER_ASYNC(cuMemcpyPeerAsync_ptsz)

CUresult cuMemcpy2D_v2(const CUDA_MEMCPY2D * p) { return 0; }
CUresult cuMemcpy2D_v2_ptds(const CUDA_MEMCPY2D * p) { return 0; }
CUresult cuMemcpy2DAsync_v2(const CUDA_MEMCPY2D * p, CUstream s) { return 0; }
CUresult cuMemcpy2DAsync_v2_ptsz(const CUDA_MEMCPY2D * p, CUstream s) { return 0; }
CUresult cuMemcpy3D_v2(const CUDA_MEMCPY3D * p) { return 0; }
CUresult cuMemcpy3D_v2_ptds(const CUDA_MEMCPY3D * p) { return 0; }
CUresult cuMemcpy3DAsync_v2(const CUDA_MEMCPY3D * p, CUstream s) { return 0; }
CUresult cuMemcpy3DAsync_v2_ptsz(const CUDA_MEMCPY3D * p, CUstream s) { return 0; }

CUresult cuStreamSynchronize(CUstream s) { return sync_sleep(0); }
CUresult cuStreamSynchronize_ptsz(CUstream s) { return sync_sleep(0); }
CUresult cuCtxSynchronize(void) { return sync_sleep(0); }
CUresult cuCtxSynchronize_v2(CUcontext c) { return sync_sleep(0); }
CUresult cuEventSynchronize(CUevent e) { return sync_sleep(600); }

CUresult cuMemAlloc_v2(CUdeviceptr * p, size_t n) { return 0; }
CUresult cuMemAllocAsync(CUdeviceptr * p, size_t n, CUstream s) { return 0; }
CUresult cuMemAllocAsync_ptsz(CUdeviceptr * p, size_t n, CUstream s) { return 0; }
CUresult cuMemAllocManaged(CUdeviceptr * p, size_t n, unsigned flags) { return 0; }
CUresult cuMemHostAlloc(void ** p, size_t n, unsigned flags) { return 0; }
CUresult cuMemAllocHost_v2(void ** p, size_t n) { return 0; }

CUresult cuMemFree_v2(CUdeviceptr p) { return 0; }
CUresult cuMemFreeAsync(CUdeviceptr p, CUstream s) { return 0; }
CUresult cuMemFreeAsync_ptsz(CUdeviceptr p, CUstream s) { return 0; }
CUresult cuMemFreeHost(void * p) { return 1; }
