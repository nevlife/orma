#undef LTTNG_UST_TRACEPOINT_PROVIDER
#define LTTNG_UST_TRACEPOINT_PROVIDER armo

#undef LTTNG_UST_TRACEPOINT_INCLUDE
#define LTTNG_UST_TRACEPOINT_INCLUDE "armo_lttng/armo_tp.h"

#if !defined(ARMO_LTTNG_ARMO_TP_H) || defined(LTTNG_UST_TRACEPOINT_HEADER_MULTI_READ)
#define ARMO_LTTNG_ARMO_TP_H

#include "armo_lttng/events.h"

#include <lttng/tracepoint.h>
#include <stdint.h>

// An event's payload is its armo_common record payload as 64-bit words, `p`, and its strings
// `s0` to `s1`: armo-collectd turns it back into the record without a table per event.

LTTNG_UST_TRACEPOINT_EVENT_CLASS(
  armo, s0, LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_sequence_hex(uint64_t, p, p_arg, uint64_t, n_arg)))

LTTNG_UST_TRACEPOINT_EVENT_CLASS(
  armo, s1, LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_sequence_hex(uint64_t, p, p_arg, uint64_t, n_arg)
                        lttng_ust_field_string(s0, s0_arg)))

LTTNG_UST_TRACEPOINT_EVENT_CLASS(
  armo, s2,
  LTTNG_UST_TP_ARGS(
    const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg, const char *, s1_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_sequence_hex(uint64_t, p, p_arg, uint64_t, n_arg)
                        lttng_ust_field_string(s0, s0_arg) lttng_ust_field_string(s1, s1_arg)))

#define ARMO_TP_X0(name, type)         \
  LTTNG_UST_TRACEPOINT_EVENT_INSTANCE( \
    armo, s0, armo, name, LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg))
#define ARMO_TP_X1(name, type, a)      \
  LTTNG_UST_TRACEPOINT_EVENT_INSTANCE( \
    armo, s1, armo, name,              \
    LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg))
#define ARMO_TP_X2(name, type, a, b)   \
  LTTNG_UST_TRACEPOINT_EVENT_INSTANCE( \
    armo, s2, armo, name,              \
    LTTNG_UST_TP_ARGS(                 \
      const uint64_t *, p_arg, uint64_t, n_arg, const char *, s0_arg, const char *, s1_arg))

ARMO_LTTNG_EVENTS(ARMO_TP_X0, ARMO_TP_X1, ARMO_TP_X2, ARMO_TP_X0, ARMO_TP_X1, ARMO_TP_X2)

LTTNG_UST_TRACEPOINT_EVENT_INSTANCE(
  armo, s0, armo, rclcpp_executor_get_next_ready,
  LTTNG_UST_TP_ARGS(const uint64_t *, p_arg, uint64_t, n_arg))

// The intercept point of armo_common event `event` found no original function.
LTTNG_UST_TRACEPOINT_EVENT(
  armo, missing, LTTNG_UST_TP_ARGS(uint16_t, event_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer(uint16_t, event, event_arg)))

#endif  // ARMO_LTTNG_ARMO_TP_H

#include <lttng/tracepoint-event.h>
