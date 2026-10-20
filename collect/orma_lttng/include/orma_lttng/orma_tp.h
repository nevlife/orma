#undef LTTNG_UST_TRACEPOINT_PROVIDER
#define LTTNG_UST_TRACEPOINT_PROVIDER orma

#undef LTTNG_UST_TRACEPOINT_INCLUDE
#define LTTNG_UST_TRACEPOINT_INCLUDE "orma_lttng/orma_tp.h"

#if !defined(ORMA_LTTNG_ORMA_TP_H) || defined(LTTNG_UST_TRACEPOINT_HEADER_MULTI_READ)
#define ORMA_LTTNG_ORMA_TP_H

#include "orma_lttng/events.h"

#include <lttng/tracepoint.h>
#include <stdint.h>

// An event's payload is its orma_common record payload as 64-bit words, `p`, and its strings
// `s0` to `s1`: orma-collectd turns it back into the record without a table per event.

LTTNG_UST_TRACEPOINT_EVENT_CLASS(
  orma, s0, LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_sequence_hex(uint64_t, p, p_arg, uint64_t, n_arg)))

LTTNG_UST_TRACEPOINT_EVENT_CLASS(
  orma, s1, LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_sequence_hex(uint64_t, p, p_arg, uint64_t, n_arg)
                        lttng_ust_field_string(s0, s0_arg)))

LTTNG_UST_TRACEPOINT_EVENT_CLASS(
  orma, s2,
  LTTNG_UST_TP_ARGS(
    const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg, const char *, s1_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_sequence_hex(uint64_t, p, p_arg, uint64_t, n_arg)
                        lttng_ust_field_string(s0, s0_arg) lttng_ust_field_string(s1, s1_arg)))

#define ORMA_TP_X0(name, type)         \
  LTTNG_UST_TRACEPOINT_EVENT_INSTANCE( \
    orma, s0, orma, name, LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg))
#define ORMA_TP_X1(name, type, a)      \
  LTTNG_UST_TRACEPOINT_EVENT_INSTANCE( \
    orma, s1, orma, name,              \
    LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg))
#define ORMA_TP_X2(name, type, a, b)   \
  LTTNG_UST_TRACEPOINT_EVENT_INSTANCE( \
    orma, s2, orma, name,              \
    LTTNG_UST_TP_ARGS(                 \
      const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg, const char *, s1_arg))

ORMA_LTTNG_EVENTS(ORMA_TP_X0, ORMA_TP_X1, ORMA_TP_X2, ORMA_TP_X0, ORMA_TP_X1, ORMA_TP_X2)

LTTNG_UST_TRACEPOINT_EVENT_INSTANCE(
  orma, s0, orma, rclcpp_executor_get_next_ready,
  LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg))

// The intercept point of orma_common event `event` found no original function.
LTTNG_UST_TRACEPOINT_EVENT(
  orma, missing, LTTNG_UST_TP_ARGS(uint16_t, event_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer(uint16_t, event, event_arg)))

#endif  // ORMA_LTTNG_ORMA_TP_H

#include <lttng/tracepoint-event.h>
