// Stands in for libcuda.so.1 (built with that soname): every function armo times, each recording
// that it was reached, and cuGetProcAddress answering a base name with the variant for the version
// and the per-thread flag, as the driver does. Linked -Bsymbolic so that its table holds its own
// functions, not the ones armo exports under the same names.
#include "armo_preload/cuda_api.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

#define STUB __attribute__((visibility("default"), noinline))

static const char * last;
static unsigned long calls;

static CUresult reached(const char * name)
{
  last = name;
  calls++;
  if (strcmp(name, "cuStreamSynchronize") == 0) {
    struct timespec ms2 = {0, 2000000};
    nanosleep(&ms2, NULL);
  }
  // CUDA_ERROR_OUT_OF_MEMORY
  return strcmp(name, "cuMemAlloc_v2") == 0 ? 2 : 0;
}

STUB const char * stub_last(void)
{
  return last;
}

STUB unsigned long stub_calls(void)
{
  return calls;
}

#define FAKE(name, api, bytes, params, args) \
  STUB CUresult name params                  \
  {                                          \
    return reached(#name);                   \
  }

ARMO_CUDA_FUNCTIONS(FAKE)

STUB CUresult cuGetProcAddress(const char * symbol, void ** pfn, int version, uint64_t flags);
STUB CUresult
cuGetProcAddress_v2(const char * symbol, void ** pfn, int version, uint64_t flags, int * status);

struct entry
{
  const char * name;
  void * f;
};

#define ENTRY(name, api, bytes, params, args) {#name, (void *)name},

static const struct entry entries[] = {
  ARMO_CUDA_FUNCTIONS(ENTRY){"cuGetProcAddress", (void *)cuGetProcAddress},
  {"cuGetProcAddress_v2", (void *)cuGetProcAddress_v2},
};

static void * find(const char * name)
{
  for (size_t i = 0; i < sizeof entries / sizeof entries[0]; i++) {
    if (strcmp(entries[i].name, name) == 0) return entries[i].f;
  }
  return NULL;
}

// The _v2 of most functions dates from CUDA 3.2; these two gained one later.
static int v2_since(const char * symbol)
{
  if (strcmp(symbol, "cuCtxSynchronize") == 0) return 13000;
  if (strcmp(symbol, "cuGetProcAddress") == 0) return 12000;
  return 3020;
}

STUB CUresult cuGetProcAddress(const char * symbol, void ** pfn, int version, uint64_t flags)
{
  static const char * const per_thread[] = {"_v2_ptds", "_v2_ptsz", "_ptds", "_ptsz"};
  char name[96];
  *pfn = NULL;
  for (int i = 0; i < 4 && *pfn == NULL && (flags & ARMO_CU_PER_THREAD) != 0; i++) {
    snprintf(name, sizeof name, "%s%s", symbol, per_thread[i]);
    *pfn = find(name);
  }
  if (*pfn == NULL && version >= v2_since(symbol)) {
    snprintf(name, sizeof name, "%s_v2", symbol);
    *pfn = find(name);
  }
  if (*pfn == NULL) *pfn = find(symbol);
  return *pfn != NULL ? 0 : CUDA_ERROR_NOT_FOUND;
}

STUB CUresult
cuGetProcAddress_v2(const char * symbol, void ** pfn, int version, uint64_t flags, int * status)
{
  CUresult r = cuGetProcAddress(symbol, pfn, version, flags);
  // CU_GET_PROC_ADDRESS_SUCCESS, CU_GET_PROC_ADDRESS_SYMBOL_NOT_FOUND
  if (status != NULL) *status = r == 0 ? 0 : 1;
  return r;
}
