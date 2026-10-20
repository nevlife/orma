// Stands in for liborma_preload.so in LD_PRELOAD: counts the markers and keeps their keys.
#include <stdint.h>

#define EXPORT __attribute__((visibility("default")))

static uint64_t begins, ends, begin_key, end_key;

EXPORT void orma_span_begin(const char * name)
{
  begins++;
  begin_key = (uint64_t)(uintptr_t)name;
}

EXPORT void orma_span_end(const char * name)
{
  ends++;
  end_key = (uint64_t)(uintptr_t)name;
}

EXPORT void fake_shim_read(uint64_t out[4])
{
  out[0] = begins;
  out[1] = ends;
  out[2] = begin_key;
  out[3] = end_key;
}
