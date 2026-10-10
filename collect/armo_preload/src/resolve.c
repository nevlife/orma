#define _GNU_SOURCE
#include "armo_preload/armo_emit.h"
#include "armo_preload/intercept.h"

#include <dlfcn.h>
#include <link.h>
#include <stddef.h>

// Objects taken per walk of the loaded list: the caller's thread may have a small stack.
#define CHUNK 32

struct objects
{
  const char * names[CHUNK];
  int skip;
  int seen;
  int n;
};

static int collect(struct dl_phdr_info * info, size_t size, void * arg)
{
  (void)size;
  struct objects * o = arg;
  if (info->dlpi_name[0] == '\0' || o->seen++ < o->skip) return 0;
  if (o->n == CHUNK) return 1;
  o->names[o->n++] = info->dlpi_name;
  return 0;
}

static void * lookup(const char * lib, const char * name)
{
  void * handle = dlopen(lib, RTLD_LAZY | RTLD_NOLOAD);
  if (handle == NULL) return NULL;
  void * f = dlsym(handle, name);
  dlclose(handle);
  return f;
}

// The first loaded object, in load order, that defines `name`, other than this library. Not the
// caller's library: a tail call leaves the caller's caller in the return address. dlopen runs
// after the walk, which holds the loader's list lock.
static void * search_loaded(const char * name)
{
  Dl_info self;
  dladdr((void *)search_loaded, &self);
  for (int skip = 0;; skip += CHUNK) {
    struct objects o = {.skip = skip};
    dl_iterate_phdr(collect, &o);
    for (int i = 0; i < o.n; i++) {
      void * f = lookup(o.names[i], name);
      Dl_info at;
      if (f != NULL && dladdr(f, &at) != 0 && at.dli_fbase != self.dli_fbase) return f;
    }
    if (o.n < CHUNK) return NULL;
  }
}

extern const struct armo_next __start_armo_next[];
extern const struct armo_next __stop_armo_next[];

// Binds at load what RTLD_NEXT finds now, so that a first call on a real-time thread does not
// take the loader's lock and allocate in dlsym. What is not loaded yet (Python loads librcl,
// libtracetools and libflux2_ffi later, RTLD_LOCAL) still resolves on its first call, which
// also marks what is missing.
__attribute__((constructor)) static void bind_loaded(void)
{
  for (const struct armo_next * e = __start_armo_next; e < __stop_armo_next; e++) {
    void * f = dlsym(RTLD_NEXT, e->name);
    if (f != NULL) __atomic_store_n(e->cache, f, __ATOMIC_RELEASE);
  }
}

void * armo_find(const char * name)
{
  void * f = dlsym(RTLD_NEXT, name);
  return f != NULL ? f : search_loaded(name);
}

void * armo_resolve(void ** cache, const char * name, const char * lib, uint16_t event)
{
  void * f = lib != NULL ? dlsym(RTLD_NEXT, name) : armo_find(name);
  if (f == NULL && lib != NULL) f = lookup(lib, name);
  if (f == NULL) {
    armo_backend_missing(event);
    f = ARMO_MISSING;
  }
  __atomic_store_n(cache, f, __ATOMIC_RELEASE);

  static int reported;
  if (!__atomic_exchange_n(&reported, 1, __ATOMIC_RELAXED)) armo_rclcpp_report_unbound();
  return f;
}
