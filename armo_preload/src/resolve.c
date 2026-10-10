#define _GNU_SOURCE
#include "armo_preload/armo_emit.h"
#include "armo_preload/intercept.h"

#include <dlfcn.h>
#include <link.h>
#include <stddef.h>

#define MAX_OBJECTS 1024

struct objects
{
  const char * names[MAX_OBJECTS];
  int n;
};

static int collect(struct dl_phdr_info * info, size_t size, void * arg)
{
  (void)size;
  struct objects * o = arg;
  if (info->dlpi_name[0] != '\0' && o->n < MAX_OBJECTS) o->names[o->n++] = info->dlpi_name;
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
  struct objects o = {.n = 0};
  dl_iterate_phdr(collect, &o);
  Dl_info self;
  dladdr((void *)search_loaded, &self);
  for (int i = 0; i < o.n; i++) {
    void * f = lookup(o.names[i], name);
    Dl_info at;
    if (f != NULL && dladdr(f, &at) != 0 && at.dli_fbase != self.dli_fbase) return f;
  }
  return NULL;
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
