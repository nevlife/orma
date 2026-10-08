#undef LTTNG_UST_TRACEPOINT_PROVIDER
#define LTTNG_UST_TRACEPOINT_PROVIDER flux2

#undef LTTNG_UST_TRACEPOINT_INCLUDE
#define LTTNG_UST_TRACEPOINT_INCLUDE "flux2_tp.h"

#if !defined(ARMO_FLUX2_TP_H) || defined(LTTNG_UST_TRACEPOINT_HEADER_MULTI_READ)
#define ARMO_FLUX2_TP_H

#include <lttng/tracepoint.h>
#include <stdint.h>

// One event per flux2_trace_* function (flux2_core/src/trace.rs), its arguments as fields.

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, channel_init,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, dev_arg, uint64_t, ino_arg, const char *, segment_arg,
    int32_t, publisher_arg, uint32_t, slot_count_arg, uint32_t, slot_size_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
      lttng_ust_field_integer(uint64_t, dev, dev_arg)
        lttng_ust_field_integer(uint64_t, ino, ino_arg)
          lttng_ust_field_string(segment, segment_arg)
            lttng_ust_field_integer(int32_t, publisher, publisher_arg)
              lttng_ust_field_integer(uint32_t, slot_count, slot_count_arg)
                lttng_ust_field_integer(uint32_t, slot_size, slot_size_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, commit,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, ticket_arg, uint32_t, slot_arg, uint64_t, nbytes_arg,
    int32_t, woke_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
      lttng_ust_field_integer(uint64_t, ticket, ticket_arg)
        lttng_ust_field_integer(uint32_t, slot, slot_arg)
          lttng_ust_field_integer(uint64_t, nbytes, nbytes_arg)
            lttng_ust_field_integer(int32_t, woke, woke_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, take,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, ticket_arg, uint32_t, slot_arg, uint64_t, lost_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
      lttng_ust_field_integer(uint64_t, ticket, ticket_arg)
        lttng_ust_field_integer(uint32_t, slot, slot_arg)
          lttng_ust_field_integer(uint64_t, lost, lost_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, refused, LTTNG_UST_TP_ARGS(const void *, channel_arg, int32_t, reason_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
      lttng_ust_field_integer(int32_t, reason, reason_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, wake, LTTNG_UST_TP_ARGS(const void *, executor_arg, uint32_t, events_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
      lttng_ust_field_integer(uint32_t, events, events_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, callback_start, LTTNG_UST_TP_ARGS(const void *, executor_arg, uint32_t, entry_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
      lttng_ust_field_integer(uint32_t, entry, entry_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, callback_end,
  LTTNG_UST_TP_ARGS(
    const void *, executor_arg, uint32_t, entry_arg, int32_t, delivered_arg, const void *,
    channel_arg, const char *, segment_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
      lttng_ust_field_integer(uint32_t, entry, entry_arg)
        lttng_ust_field_integer(int32_t, delivered, delivered_arg)
          lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
            lttng_ust_field_string(segment, segment_arg)))

#endif  // ARMO_FLUX2_TP_H

#include <lttng/tracepoint-event.h>
