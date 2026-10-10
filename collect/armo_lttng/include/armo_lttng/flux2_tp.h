#undef LTTNG_UST_TRACEPOINT_PROVIDER
#define LTTNG_UST_TRACEPOINT_PROVIDER flux2

#undef LTTNG_UST_TRACEPOINT_INCLUDE
#define LTTNG_UST_TRACEPOINT_INCLUDE "armo_lttng/flux2_tp.h"

#if !defined(ARMO_FLUX2_TP_H) || defined(LTTNG_UST_TRACEPOINT_HEADER_MULTI_READ)
#define ARMO_FLUX2_TP_H

#include <lttng/tracepoint.h>
#include <stdint.h>

// One event per flux2_trace_* function of trace contract 2 (flux2/trace.h), its arguments as
// fields of the same names.

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, channel_init,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, dev_arg, uint64_t, ino_arg, const char *, segment_arg,
    int32_t, publisher_arg, uint32_t, slot_count_arg, uint32_t, slot_size_arg, int32_t,
    stamp_offset_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint64_t, dev, dev_arg) lttng_ust_field_integer(
                          uint64_t, ino, ino_arg) lttng_ust_field_string(segment, segment_arg)
                          lttng_ust_field_integer(int32_t, publisher, publisher_arg)
                            lttng_ust_field_integer(uint32_t, slot_count, slot_count_arg)
                              lttng_ust_field_integer(uint32_t, slot_size, slot_size_arg)
                                lttng_ust_field_integer(int32_t, stamp_offset, stamp_offset_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, claim,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint32_t, slot_arg, int32_t, result_arg, int32_t, loan_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint32_t, slot, slot_arg)
                          lttng_ust_field_integer(int32_t, result, result_arg)
                            lttng_ust_field_integer(int32_t, loan, loan_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, loan, LTTNG_UST_TP_ARGS(const void *, channel_arg, uint32_t, slot_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint32_t, slot, slot_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, loan_end,
  LTTNG_UST_TP_ARGS(const void *, channel_arg, uint32_t, slot_arg, int32_t, result_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint32_t, slot, slot_arg)
                          lttng_ust_field_integer(int32_t, result, result_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, commit,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, ticket_arg, uint32_t, slot_arg, uint64_t, nbytes_arg,
    int32_t, woke_arg, uint64_t, stamp_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint64_t, ticket, ticket_arg)
                          lttng_ust_field_integer(uint32_t, slot, slot_arg)
                            lttng_ust_field_integer(uint64_t, nbytes, nbytes_arg)
                              lttng_ust_field_integer(int32_t, woke, woke_arg)
                                lttng_ust_field_integer(uint64_t, stamp, stamp_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, take,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, ticket_arg, uint32_t, slot_arg, uint64_t, lost_arg,
    uint64_t, stamp_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint64_t, ticket, ticket_arg)
                          lttng_ust_field_integer(uint32_t, slot, slot_arg)
                            lttng_ust_field_integer(uint64_t, lost, lost_arg)
                              lttng_ust_field_integer(uint64_t, stamp, stamp_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, release,
  LTTNG_UST_TP_ARGS(const void *, channel_arg, uint64_t, ticket_arg, int32_t, held_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint64_t, ticket, ticket_arg)
                          lttng_ust_field_integer(int32_t, held, held_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, refused, LTTNG_UST_TP_ARGS(const void *, channel_arg, int32_t, reason_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(int32_t, reason, reason_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, wait,
  LTTNG_UST_TP_ARGS(const void *, executor_arg, uint32_t, sources_arg, int64_t, timeout_ns_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
                        lttng_ust_field_integer(uint32_t, sources, sources_arg)
                          lttng_ust_field_integer(int64_t, timeout_ns, timeout_ns_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, wake,
  LTTNG_UST_TP_ARGS(const void *, executor_arg, int32_t, result_arg, uint32_t, entry_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
                        lttng_ust_field_integer(int32_t, result, result_arg)
                          lttng_ust_field_integer(uint32_t, entry, entry_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, callback_start,
  LTTNG_UST_TP_ARGS(const void *, executor_arg, uint32_t, entry_arg, const void *, endpoint_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
      lttng_ust_field_integer(uint32_t, entry, entry_arg)
        lttng_ust_field_integer_hex(uintptr_t, endpoint, (uintptr_t)endpoint_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, callback_end,
  LTTNG_UST_TP_ARGS(
    const void *, executor_arg, uint32_t, entry_arg, int32_t, delivered_arg, const void *,
    channel_arg, const char *, segment_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, executor, (uintptr_t)executor_arg)
                        lttng_ust_field_integer(uint32_t, entry, entry_arg)
                          lttng_ust_field_integer(int32_t, delivered, delivered_arg)
                            lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                              lttng_ust_field_string(segment, segment_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, endpoint_init,
  LTTNG_UST_TP_ARGS(
    const void *, endpoint_arg, const char *, node_arg, const char *, key_arg, const char *,
    signpost_arg, uint64_t, fingerprint_arg, int32_t, role_arg, int32_t, lang_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, endpoint, (uintptr_t)endpoint_arg)
                        lttng_ust_field_string(node, node_arg) lttng_ust_field_string(key, key_arg)
                          lttng_ust_field_string(signpost, signpost_arg)
                            lttng_ust_field_integer(uint64_t, fingerprint, fingerprint_arg)
                              lttng_ust_field_integer(int32_t, role, role_arg)
                                lttng_ust_field_integer(int32_t, lang, lang_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, py_gil, LTTNG_UST_TP_ARGS(int32_t, phase_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer(int32_t, phase, phase_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, py_ready, LTTNG_UST_TP_ARGS(const void *, channel_arg, uint64_t, ticket_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint64_t, ticket, ticket_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, record_drop,
  LTTNG_UST_TP_ARGS(
    const void *, channel_arg, uint64_t, ticket_arg, uint32_t, count_arg, int32_t, reason_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(uintptr_t, channel, (uintptr_t)channel_arg)
                        lttng_ust_field_integer(uint64_t, ticket, ticket_arg)
                          lttng_ust_field_integer(uint32_t, count, count_arg)
                            lttng_ust_field_integer(int32_t, reason, reason_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  flux2, gpu_wedged, LTTNG_UST_TP_ARGS(int32_t, result_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer(int32_t, result, result_arg)))

#endif  // ARMO_FLUX2_TP_H

#include <lttng/tracepoint-event.h>
