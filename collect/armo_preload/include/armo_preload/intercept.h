// armo_intercept internals: exports, and the next definition of a caught function.
#ifndef ARMO_PRELOAD_INTERCEPT_H
#define ARMO_PRELOAD_INTERCEPT_H

#include <stdint.h>
#include <string.h>

#define ARMO_EXPORT __attribute__((visibility("default")))
#define ARMO_U64(p) ((uint64_t)(uintptr_t)(p))

// The next definition of `name`: RTLD_NEXT, else in the loaded `lib`, else (`lib` NULL) in any
// loaded object, since Python loads librcl, libtracetools and libflux2_ffi RTLD_LOCAL. Cached in
// `cache`; ARMO_MISSING after marking `event` missing.
void * armo_resolve(void ** cache, const char * name, const char * lib, uint16_t event);
void * armo_find(const char * name);

#define ARMO_MISSING ((void *)-1)

// A call site's cache, for binding at load what RTLD_NEXT already finds (resolve.c).
struct armo_next
{
  void ** cache;
  const char * name;
};

void armo_rclcpp_report_unbound(void);

#define ARMO_NEXT(fn, lib, event)                                   \
  ({                                                                \
    static void * cache_;                                           \
    static const struct armo_next next_                             \
      __attribute__((used, section("armo_next"))) = {&cache_, #fn}; \
    void * f_ = __atomic_load_n(&cache_, __ATOMIC_RELAXED);         \
    if (f_ == NULL) f_ = armo_resolve(&cache_, #fn, lib, event);    \
    (__typeof__(&fn))(f_ == ARMO_MISSING ? NULL : f_);              \
  })

static inline uint64_t armo_stamp(const void * message)
{
  uint64_t s = 0;
  if (message != NULL) memcpy(&s, message, sizeof s);
  return s;
}

#endif  // ARMO_PRELOAD_INTERCEPT_H
