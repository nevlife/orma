// The DDS write under rmw_publish (design 20.2), timed while the header's options ask for it:
// Cyclone DDS's C functions (rmw_cyclonedds calls dds_write, dds_write_ts and dds_forwardcdr), and
// Fast DDS's DataWriter::write in the overloads rmw_fastrtps calls (with WriteParams) and the plain
// ones, of Fast DDS 2 (void *) and 3 (const void *). Fast DDS returns bool in 2 and ReturnCode_t in
// 3; the wrapper passes the return register through.
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"

#include <stdint.h>

#define DDS_RETCODE_ERROR (-1)
#define FASTDDS2_FALSE 0
#define FASTDDS3_RETCODE_ERROR 1

typedef int32_t dds_return_t;
typedef int32_t dds_entity_t;
typedef int64_t dds_time_t;
struct ddsi_serdata;

static void wrote(uint64_t writer, uint64_t t0)
{
  orma_ev_dds_write ev = {.writer = writer, .duration_ns = orma_now() - t0};
  orma_emit_dds_write(&ev);
}

#define CYCLONE_POINT(name, params, args)                                  \
  ORMA_EXPORT dds_return_t name params;                                    \
  dds_return_t name params                                                 \
  {                                                                        \
    dds_return_t(*next) params = ORMA_NEXT(name, NULL, ORMA_EV_DDS_WRITE); \
    if (next == NULL) return DDS_RETCODE_ERROR;                            \
    if (!orma_backend_dds_on()) return next args;                          \
    uint64_t t0 = orma_now();                                              \
    dds_return_t r = next args;                                            \
    wrote((uint32_t)writer, t0);                                           \
    return r;                                                              \
  }

CYCLONE_POINT(dds_write, (dds_entity_t writer, const void * data), (writer, data))
CYCLONE_POINT(
  dds_write_ts, (dds_entity_t writer, const void * data, dds_time_t timestamp),
  (writer, data, timestamp))
CYCLONE_POINT(dds_writecdr, (dds_entity_t writer, struct ddsi_serdata * serdata), (writer, serdata))
CYCLONE_POINT(
  dds_forwardcdr, (dds_entity_t writer, struct ddsi_serdata * serdata), (writer, serdata))

#define FASTDDS_POINT(id, symbol, fail, params, args)                                   \
  ORMA_EXPORT uintptr_t id params __asm__(symbol);                                      \
  uintptr_t id params                                                                   \
  {                                                                                     \
    uintptr_t(*next) params =                                                           \
      ORMA_NEXT_BY(uintptr_t(*) params, symbol, orma_resolve, NULL, ORMA_EV_DDS_WRITE); \
    if (next == NULL) return fail;                                                      \
    if (!orma_backend_dds_on()) return next args;                                       \
    uint64_t t0 = orma_now();                                                           \
    uintptr_t r = next args;                                                            \
    wrote(self, t0);                                                                    \
    return r;                                                                           \
  }

FASTDDS_POINT(
  fastdds2_write, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPv", FASTDDS2_FALSE,
  (uintptr_t self, uintptr_t data), (self, data))
FASTDDS_POINT(
  fastdds2_write_params,
  "_ZN8eprosima7fastdds3dds10DataWriter5writeEPvRNS_8fastrtps4rtps11WriteParamsE", FASTDDS2_FALSE,
  (uintptr_t self, uintptr_t data, uintptr_t params), (self, data, params))
FASTDDS_POINT(
  fastdds3_write, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPKv", FASTDDS3_RETCODE_ERROR,
  (uintptr_t self, uintptr_t data), (self, data))
FASTDDS_POINT(
  fastdds3_write_params, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPKvRNS0_4rtps11WriteParamsE",
  FASTDDS3_RETCODE_ERROR, (uintptr_t self, uintptr_t data, uintptr_t params), (self, data, params))
