// malloc attribution (design 15.3), opt-in at run time through the header's options. While off,
// a caught call costs the backend's check of that word. The allocator is found with RTLD_NEXT so
// that one preloaded after armo still serves; dlsym may allocate while it looks, and those few
// allocations come from glibc's own entry points.
#define _GNU_SOURCE
#include "armo_preload/armo_emit.h"
#include "armo_preload/intercept.h"

#include <dlfcn.h>
#include <errno.h>
#include <malloc.h>
#include <stddef.h>

void * __libc_malloc(size_t size);
void * __libc_calloc(size_t n, size_t size);
void * __libc_realloc(void * p, size_t size);
void * __libc_memalign(size_t alignment, size_t size);
void __libc_free(void * p);

static struct
{
  void * (*malloc)(size_t);
  void * (*calloc)(size_t, size_t);
  void * (*realloc)(void *, size_t);
  void (*free)(void *);
  int (*posix_memalign)(void **, size_t, size_t);
  void * (*aligned_alloc)(size_t, size_t);
  int ready;
} next;

static __thread int resolving __attribute__((tls_model("initial-exec")));

static int resolved(void)
{
  if (__atomic_load_n(&next.ready, __ATOMIC_ACQUIRE)) return 1;
  if (resolving) return 0;
  resolving = 1;
  next.malloc = dlsym(RTLD_NEXT, "malloc");
  next.calloc = dlsym(RTLD_NEXT, "calloc");
  next.realloc = dlsym(RTLD_NEXT, "realloc");
  next.free = dlsym(RTLD_NEXT, "free");
  next.posix_memalign = dlsym(RTLD_NEXT, "posix_memalign");
  next.aligned_alloc = dlsym(RTLD_NEXT, "aligned_alloc");
  __atomic_store_n(&next.ready, 1, __ATOMIC_RELEASE);
  resolving = 0;
  return 1;
}

static void * allocated(void * p)
{
  if (p != NULL && armo_backend_malloc_on()) armo_backend_alloc(malloc_usable_size(p));
  return p;
}

static void releasing(void * p)
{
  if (p != NULL && armo_backend_malloc_on()) armo_backend_free(malloc_usable_size(p));
}

ARMO_EXPORT void * malloc(size_t size)
{
  if (!resolved()) return __libc_malloc(size);
  return allocated(next.malloc(size));
}

ARMO_EXPORT void * calloc(size_t n, size_t size)
{
  if (!resolved()) return __libc_calloc(n, size);
  return allocated(next.calloc(n, size));
}

ARMO_EXPORT void * realloc(void * p, size_t size)
{
  if (!resolved()) return __libc_realloc(p, size);
  releasing(p);
  return allocated(next.realloc(p, size));
}

ARMO_EXPORT void free(void * p)
{
  if (!resolved()) {
    __libc_free(p);
    return;
  }
  releasing(p);
  next.free(p);
}

ARMO_EXPORT int posix_memalign(void ** out, size_t alignment, size_t size)
{
  if (!resolved()) {
    *out = __libc_memalign(alignment, size);
    return *out != NULL ? 0 : ENOMEM;
  }
  int ret = next.posix_memalign(out, alignment, size);
  if (ret == 0) allocated(*out);
  return ret;
}

ARMO_EXPORT void * aligned_alloc(size_t alignment, size_t size)
{
  if (!resolved()) return __libc_memalign(alignment, size);
  return allocated(next.aligned_alloc(alignment, size));
}
