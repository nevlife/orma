"""Calls every CUDA driver function ormad attaches to in a stand-in libcuda.so.1 (fake_cuda.c), a
known number of times with known sizes, for accuracy.py's gpu case. A second thread calls
cuCtxSynchronize meanwhile. Prints "pid worker_tid", waits for "go", calls, prints the records
it expects as JSON [[api, bytes, result, count], ...], and waits for a line before it exits.

    cuda_caller.py LIBCUDA TIMES
"""

import collections
import ctypes
import json
import os
import sys
import threading
import time

lib = ctypes.CDLL(sys.argv[1])
times = int(sys.argv[2])
V, U64, SZ, U = ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t, ctypes.c_uint
LAUNCH, GRAPH, MEMCPY, MEMCPY_ASYNC, SYNC_STREAM, SYNC_CTX, SYNC_EVENT, ALLOC, FREE = range(9)
RESULT = {"cuEventSynchronize": 600, "cuMemFreeHost": 1}
expect = collections.Counter()


def memcpy(base, peer=False, stream=False):
    def call(f, k):
        n = base + k
        args = [U64(1), V(2), U64(3), V(4), SZ(n)] if peer else [U64(1), U64(2), SZ(n)]
        f(*args, *([V(5)] if stream else []))
        return n

    return call


def copy_2d(f, k):
    desc = (U64 * 16)()
    desc[14], desc[15] = 64, 8 + k
    f(ctypes.byref(desc), V(5))
    return 64 * (8 + k)


def copy_3d(f, k):
    desc = (U64 * 25)()
    desc[22], desc[23], desc[24] = 16, 4, 2 + k
    f(ctypes.byref(desc), V(5))
    return 16 * 4 * (2 + k)


def alloc(f, k):
    out = U64()
    n = (1 << 20) + k
    f(ctypes.byref(out), SZ(n), V(0))
    return n


def plain(f, k):
    f(V(0), U(1), U(1), U(1), U(1), U(1), U(1), U(0), V(0), V(0), V(0))
    return 0


GROUPS = [
    (LAUNCH, plain, "cuLaunchKernel cuLaunchKernel_ptsz cuLaunchKernelEx cuLaunchKernelEx_ptsz"),
    (LAUNCH, plain, "cuLaunchCooperativeKernel cuLaunchCooperativeKernel_ptsz"),
    (GRAPH, plain, "cuGraphLaunch cuGraphLaunch_ptsz"),
    (
        MEMCPY,
        memcpy(4096),
        "cuMemcpy cuMemcpy_ptds cuMemcpyHtoD_v2 cuMemcpyHtoD_v2_ptds cuMemcpyDtoH_v2 "
        "cuMemcpyDtoH_v2_ptds cuMemcpyDtoD_v2 cuMemcpyDtoD_v2_ptds",
    ),
    (
        MEMCPY_ASYNC,
        memcpy(8192, stream=True),
        "cuMemcpyAsync cuMemcpyAsync_ptsz cuMemcpyHtoDAsync_v2 cuMemcpyHtoDAsync_v2_ptsz "
        "cuMemcpyDtoHAsync_v2 cuMemcpyDtoHAsync_v2_ptsz cuMemcpyDtoDAsync_v2 "
        "cuMemcpyDtoDAsync_v2_ptsz",
    ),
    (MEMCPY, memcpy(12288, peer=True), "cuMemcpyPeer cuMemcpyPeer_ptds"),
    (MEMCPY_ASYNC, memcpy(16384, True, True), "cuMemcpyPeerAsync cuMemcpyPeerAsync_ptsz"),
    (MEMCPY, copy_2d, "cuMemcpy2D_v2 cuMemcpy2D_v2_ptds"),
    (MEMCPY_ASYNC, copy_2d, "cuMemcpy2DAsync_v2 cuMemcpy2DAsync_v2_ptsz"),
    (MEMCPY, copy_3d, "cuMemcpy3D_v2 cuMemcpy3D_v2_ptds"),
    (MEMCPY_ASYNC, copy_3d, "cuMemcpy3DAsync_v2 cuMemcpy3DAsync_v2_ptsz"),
    (SYNC_STREAM, plain, "cuStreamSynchronize cuStreamSynchronize_ptsz"),
    (SYNC_CTX, plain, "cuCtxSynchronize_v2"),
    (SYNC_EVENT, plain, "cuEventSynchronize"),
    (
        ALLOC,
        alloc,
        "cuMemAlloc_v2 cuMemAllocAsync cuMemAllocAsync_ptsz cuMemAllocManaged cuMemHostAlloc "
        "cuMemAllocHost_v2",
    ),
    (FREE, plain, "cuMemFree_v2 cuMemFreeAsync cuMemFreeAsync_ptsz cuMemFreeHost"),
]


def worker(go, tids):
    tids.append(threading.get_native_id())
    go.wait()
    for _ in range(times):
        lib.cuCtxSynchronize()


go, tids = threading.Event(), []
other = threading.Thread(target=worker, args=(go, tids))
other.start()
while not tids:
    time.sleep(0.01)
print(os.getpid(), tids[0], flush=True)
sys.stdin.readline()
go.set()
for api, call, names in GROUPS:
    for k, name in enumerate(names.split()):
        f = getattr(lib, name)
        for _ in range(times):
            expect[(api, call(f, k), RESULT.get(name, 0))] += 1
other.join()
expect[(SYNC_CTX, 0, 0)] += times
print(json.dumps([[*k, n] for k, n in expect.items()]), flush=True)
sys.stdin.readline()
