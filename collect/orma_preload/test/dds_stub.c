// Stands in for libddsc.so and libfastrtps.so / libfastdds.so: the write functions orma times,
// each recording that it was reached and returning what the real one returns on success (Fast DDS
// 2's DataWriter::write true, 3's RETCODE_OK).
#include <stdint.h>

#define STUB __attribute__((visibility("default"), noinline))

static const char * last;

STUB const char * stub_last(void)
{
  return last;
}

#define CYCLONE(name)                                  \
  STUB int32_t name(int32_t writer, const void * data) \
  {                                                    \
    (void)writer, (void)data;                          \
    last = #name;                                      \
    return 0;                                          \
  }

CYCLONE(dds_write)

STUB int32_t dds_write_ts(int32_t writer, const void * data, int64_t timestamp)
{
  (void)writer, (void)data, (void)timestamp;
  last = "dds_write_ts";
  return 0;
}

CYCLONE(dds_writecdr)
CYCLONE(dds_forwardcdr)

#define FASTDDS(id, symbol, ok)                                                        \
  STUB uintptr_t id(uintptr_t self, uintptr_t data, uintptr_t params) __asm__(symbol); \
  uintptr_t id(uintptr_t self, uintptr_t data, uintptr_t params)                       \
  {                                                                                    \
    (void)self, (void)data, (void)params;                                              \
    last = symbol;                                                                     \
    return ok;                                                                         \
  }

FASTDDS(fastdds2_write, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPv", 1)
FASTDDS(
  fastdds2_write_params,
  "_ZN8eprosima7fastdds3dds10DataWriter5writeEPvRNS_8fastrtps4rtps11WriteParamsE", 1)
FASTDDS(fastdds3_write, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPKv", 0)
FASTDDS(
  fastdds3_write_params, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPKvRNS0_4rtps11WriteParamsE",
  0)
