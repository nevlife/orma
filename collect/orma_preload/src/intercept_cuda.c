// CUDA driver functions of libcuda.so.1 (design 20.1), timed while the header's options ask for
// it. cudart (11.3 on, static or shared) opens libcuda itself, takes cuGetProcAddress_v2 with dlsym
// on that handle, which no LD_PRELOAD definition interposes, and every other entry point from
// cuGetProcAddress. So dlsym on a library handle and cuGetProcAddress hand out these wrappers in
// place of the functions of the same names.
#define _GNU_SOURCE
#include "orma_preload/cuda_api.h"
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"

#include <dlfcn.h>
#include <string.h>

#define CUDA "libcuda.so.1"

typedef void * (*dlsym_fn)(void *, const char *);

// glibc's dlsym: the versioned lookup passes over the unversioned dlsym of another orma backend.
static dlsym_fn real_dlsym(void)
{
  static dlsym_fn real;
  dlsym_fn f = __atomic_load_n(&real, __ATOMIC_RELAXED);
  if (f == NULL) {
    f = (dlsym_fn)dlvsym(RTLD_NEXT, "dlsym", "GLIBC_2.34");
    __atomic_store_n(&real, f, __ATOMIC_RELAXED);
  }
  return f;
}

// Not orma_resolve: its lookups go through the dlsym below, which would hand back this wrapper.
static void * resolve(void ** cache, const char * name, const char * lib, uint16_t event)
{
  void * f = real_dlsym()(RTLD_NEXT, name);
  if (f == NULL) {
    void * handle = dlopen(lib, RTLD_LAZY | RTLD_NOLOAD);
    if (handle != NULL) {
      f = real_dlsym()(handle, name);
      dlclose(handle);
    }
  }
  if (f == NULL) {
    orma_backend_missing(event);
    f = ORMA_MISSING;
  }
  __atomic_store_n(cache, f, __ATOMIC_RELEASE);
  return f;
}

#define CUDA_NEXT(name) \
  ORMA_NEXT_BY(__typeof__(&wrap_##name), #name, resolve, CUDA, ORMA_EV_CUDA_CALL)

static void timed(uint32_t api, uint64_t bytes, CUresult result, uint64_t t0)
{
  orma_ev_cuda_call ev = {
    .duration_ns = orma_now() - t0, .bytes = bytes, .api = api, .result = result};
  orma_emit_cuda_call(&ev);
}

#define CUDA_POINT(name, api, bytes, params, args) \
  static CUresult wrap_##name params               \
  {                                                \
    CUresult(*next) params = CUDA_NEXT(name);      \
    if (next == NULL) return CUDA_ERROR_NOT_FOUND; \
    if (!orma_backend_gpu_on()) return next args;  \
    uint64_t t0 = orma_now();                      \
    CUresult r = next args;                        \
    timed(ORMA_CUDA_##api, bytes, r, t0);          \
    return r;                                      \
  }                                                \
  ORMA_EXPORT CUresult name params __attribute__((alias("wrap_" #name)));

ORMA_CUDA_FUNCTIONS(CUDA_POINT)

static CUresult wrap_cuGetProcAddress(const char *, void **, int, uint64_t);
static CUresult wrap_cuGetProcAddress_v2(const char *, void **, int, uint64_t, int *);

struct wrapper
{
  const char * name;
  void * f;
};

#define WRAPPER(name, api, bytes, params, args) {#name, (void *)wrap_##name},

static const struct wrapper wrappers[] = {
  ORMA_CUDA_FUNCTIONS(WRAPPER){"cuGetProcAddress", (void *)wrap_cuGetProcAddress},
  {"cuGetProcAddress_v2", (void *)wrap_cuGetProcAddress_v2},
};

static void * wrapper_of(const char * name)
{
  for (size_t i = 0; i < sizeof wrappers / sizeof wrappers[0]; i++) {
    if (strcmp(wrappers[i].name, name) == 0) {
      return wrappers[i].f;
    }
  }
  return NULL;
}

// The driver answers a base name with the variant for the version and flags asked; the symbol at
// the address it returns names that variant.
static void handed_out(void ** pfn)
{
  Dl_info at;
  if (
    pfn != NULL && *pfn != NULL && dladdr(*pfn, &at) != 0 && at.dli_saddr == *pfn &&
    at.dli_sname != NULL) {
    void * w = wrapper_of(at.dli_sname);
    if (w != NULL) {
      *pfn = w;
    }
  }
}

static CUresult wrap_cuGetProcAddress(const char * symbol, void ** pfn, int version, uint64_t flags)
{
  orma_cu_get_proc_address next = CUDA_NEXT(cuGetProcAddress);
  if (next == NULL) {
    return CUDA_ERROR_NOT_FOUND;
  }
  CUresult r = next(symbol, pfn, version, flags);
  if (r == 0) {
    handed_out(pfn);
  }
  return r;
}

static CUresult wrap_cuGetProcAddress_v2(
  const char * symbol, void ** pfn, int version, uint64_t flags, int * status)
{
  orma_cu_get_proc_address_v2 next = CUDA_NEXT(cuGetProcAddress_v2);
  if (next == NULL) {
    return CUDA_ERROR_NOT_FOUND;
  }
  CUresult r = next(symbol, pfn, version, flags, status);
  if (r == 0) {
    handed_out(pfn);
  }
  return r;
}

ORMA_EXPORT CUresult cuGetProcAddress(const char *, void **, int, uint64_t)
  __attribute__((alias("wrap_cuGetProcAddress")));
ORMA_EXPORT CUresult cuGetProcAddress_v2(const char *, void **, int, uint64_t, int *)
  __attribute__((alias("wrap_cuGetProcAddress_v2")));

// RTLD_NEXT is relative to the caller's object, so that path must leave as a tail call (built
// optimized, design 4.5); preload_collect checks it.
ORMA_EXPORT void * dlsym(void * __restrict handle, const char * __restrict name)
{
  if (name[0] == 'c' && name[1] == 'u' && handle != RTLD_NEXT && handle != RTLD_DEFAULT) {
    void * f = real_dlsym()(handle, name);
    void * w = f != NULL ? wrapper_of(name) : NULL;
    return w != NULL ? w : f;
  }
  return real_dlsym()(handle, name);
}
