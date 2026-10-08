// LD_PRELOAD library: defines the flux2_trace_* functions, so libflux2_ffi's calls, which go
// through its PLT, land here and become LTTng events of provider flux2. The probes live in this
// library too; a process started without it pays nothing.
#define _GNU_SOURCE
#define LTTNG_UST_TRACEPOINT_CREATE_PROBES
#define LTTNG_UST_TRACEPOINT_DEFINE
#include <dlfcn.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>

#include "flux2_tp.h"

#define ARMO_EXPORT __attribute__((visibility("default"), noinline))
#define CONTRACT "flux2_trace_contract_1"

// 0 not checked yet, 1 the calling library has the contract these events decode, -1 it has not.
static atomic_int contract;

// Asked of the library that made the call: a Python process loads libflux2_ffi RTLD_LOCAL, so a
// global lookup would not find it.
static bool agreed(const void * caller)
{
  int c = atomic_load_explicit(&contract, memory_order_relaxed);
  if (c != 0) return c > 0;
  Dl_info info;
  void * lib = dladdr(caller, &info) ? dlopen(info.dli_fname, RTLD_NOW | RTLD_NOLOAD) : NULL;
  c = lib != NULL && dlsym(lib, CONTRACT) != NULL ? 1 : -1;
  if (lib != NULL) dlclose(lib);
  int unset = 0;
  if (atomic_compare_exchange_strong(&contract, &unset, c) && c < 0) {
    fprintf(stderr, "armo_flux2_lttng: the flux2 library lacks %s; no flux2 events\n", CONTRACT);
  }
  return c > 0;
}

#define FROM_FLUX2 agreed(__builtin_return_address(0))

ARMO_EXPORT void flux2_trace_channel_init(
  const void * channel, uint64_t dev, uint64_t ino, const char * segment, int32_t publisher,
  uint32_t slot_count, uint32_t slot_size)
{
  if (FROM_FLUX2) {
    lttng_ust_tracepoint(
      flux2, channel_init, channel, dev, ino, segment, publisher, slot_count, slot_size);
  }
}

ARMO_EXPORT void flux2_trace_commit(
  const void * channel, uint64_t ticket, uint32_t slot, uint64_t nbytes, int32_t woke)
{
  if (FROM_FLUX2) lttng_ust_tracepoint(flux2, commit, channel, ticket, slot, nbytes, woke);
}

ARMO_EXPORT void flux2_trace_take(
  const void * channel, uint64_t ticket, uint32_t slot, uint64_t lost)
{
  if (FROM_FLUX2) lttng_ust_tracepoint(flux2, take, channel, ticket, slot, lost);
}

ARMO_EXPORT void flux2_trace_refused(const void * channel, int32_t reason)
{
  if (FROM_FLUX2) lttng_ust_tracepoint(flux2, refused, channel, reason);
}

ARMO_EXPORT void flux2_trace_wake(const void * executor, uint32_t events)
{
  if (FROM_FLUX2) lttng_ust_tracepoint(flux2, wake, executor, events);
}

ARMO_EXPORT void flux2_trace_callback_start(const void * executor, uint32_t entry)
{
  if (FROM_FLUX2) lttng_ust_tracepoint(flux2, callback_start, executor, entry);
}

ARMO_EXPORT void flux2_trace_callback_end(
  const void * executor, uint32_t entry, int32_t delivered, const void * channel,
  const char * segment)
{
  if (FROM_FLUX2) {
    lttng_ust_tracepoint(flux2, callback_end, executor, entry, delivered, channel, segment);
  }
}
