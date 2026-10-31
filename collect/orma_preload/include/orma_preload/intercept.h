// orma_intercept internals: exports, and the next definition of a caught function.
#ifndef ORMA_PRELOAD_INTERCEPT_H
#define ORMA_PRELOAD_INTERCEPT_H

#include <stdint.h>
#include <string.h>
#include <time.h>

#define ORMA_EXPORT __attribute__((visibility("default")))
#define ORMA_U64(p) ((uint64_t)(uintptr_t)(p))

// The next definition of `name`: RTLD_NEXT, else in the loaded `lib`, else (`lib` NULL) in any
// loaded object, since Python loads librcl, libtracetools and libflux_ffi RTLD_LOCAL. Cached in
// `cache`; ORMA_MISSING after marking `event` missing.
void * orma_resolve(void ** cache, const char * name, const char * lib, uint16_t event);
void * orma_find(const char * name);

#define ORMA_MISSING ((void *)-1)

// A call site's cache, for binding at load what RTLD_NEXT already finds (resolve.c).
struct orma_next
{
  void ** cache;
  const char * name;
};

void orma_rclcpp_report_unbound(void);

// `name` as a `type`, found on the first call by `resolve`, which has orma_resolve's signature.
#define ORMA_NEXT_BY(type, name, resolve, lib, event)                \
  ({                                                                 \
    static void * cache_;                                            \
    static const struct orma_next next_                              \
      __attribute__((used, section("orma_next"))) = {&cache_, name}; \
    void * f_ = __atomic_load_n(&cache_, __ATOMIC_RELAXED);          \
    if (f_ == NULL) f_ = resolve(&cache_, name, lib, event);         \
    (type)(f_ == ORMA_MISSING ? NULL : f_);                          \
  })

#define ORMA_NEXT(fn, lib, event) ORMA_NEXT_BY(__typeof__(&fn), #fn, orma_resolve, lib, event)

static inline uint64_t orma_stamp(const void * message)
{
  uint64_t s = 0;
  if (message != NULL) {
    memcpy(&s, message, sizeof s);
  }
  return s;
}

// For the intercepts that time the call they pass on; the backend stamps the event itself.
static inline uint64_t orma_now(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

#endif  // ORMA_PRELOAD_INTERCEPT_H
