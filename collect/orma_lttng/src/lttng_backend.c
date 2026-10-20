// The backend of liborma_lttng.so (design 3, 13.7): each orma_emit_<name> becomes one LTTng event,
// of provider orma, or of provider flux with the flux function's arguments as fields. Where
// libtracetools has no LTTng (Humble), the ROS events also go out as provider ros2 events in
// tracetools' own layout. The clock and the aggregation are orma-collectd's. The one per-thread
// state is the current callback, in a backend 2 file whose slots hold no table and no ring, for
// ormad's off-CPU attribution (design 13.2, 15.2).
// The tracepoint definitions of the three providers, once for the library.
#define LTTNG_UST_TRACEPOINT_DEFINE
#include "orma_lttng/flux_tp.h"
#include "orma_lttng/orma_tp.h"
#include "orma_preload/orma_emit.h"
#include "orma_preload/slots.h"
#if ORMA_ROS2_PROVIDER
#include "orma_lttng/ros2_tp.h"
#define ROS2(...) lttng_ust_tracepoint(ros2, __VA_ARGS__)
#else
#define ROS2(...) ((void)0)
#endif

#define WORDS(ev) (const uint64_t *)(ev), (uint64_t)(sizeof *(ev) / sizeof(uint64_t))
#define PTR(v) ((const void *)(uintptr_t)(v))

// Its own name, apart from liborma_preload.so's file when a process loads both.
const struct orma_layout orma_layout = {.backend = ORMA_BACKEND_LTTNG, .suffix = ".lttng"};

// The current callback is set after the start event and cleared before the end event, so the
// off-CPU time ormad charges to it lies within the callback's time.
static inline __attribute__((always_inline)) void entered(
  uint16_t src, uint64_t key, uint32_t entry)
{
  struct orma_thread * t = orma_thread();
  if (t != NULL) orma_push(t, src, key, entry, 0);
}

static inline __attribute__((always_inline)) void leaving(
  uint16_t src, uint64_t key, uint32_t entry)
{
  struct orma_thread * t = orma_thread();
  uint64_t t0;
  if (t != NULL) orma_pop(t, src, key, entry, &t0);
}

#define CHECK(name, type) \
  _Static_assert(sizeof(type) % sizeof(uint64_t) == 0, #type " is not whole words");
#define CHECK1(name, type, a) CHECK(name, type)
#define CHECK2(name, type, a, b) CHECK(name, type)
ORMA_LTTNG_EVENTS(CHECK, CHECK1, CHECK2, CHECK, CHECK1, CHECK2)

#define EMIT0(name, type)                        \
  void orma_emit_##name(const type * ev)         \
  {                                              \
    lttng_ust_tracepoint(orma, name, WORDS(ev)); \
  }
#define EMIT1(name, type, a)                             \
  void orma_emit_##name(const type * ev, const char * a) \
  {                                                      \
    lttng_ust_tracepoint(orma, name, WORDS(ev), a);      \
  }
#define EMIT2(name, type, a, b)                                          \
  void orma_emit_##name(const type * ev, const char * a, const char * b) \
  {                                                                      \
    lttng_ust_tracepoint(orma, name, WORDS(ev), a, b);                   \
  }
#define BY_HAND(...)
ORMA_LTTNG_EVENTS(EMIT0, EMIT1, EMIT2, BY_HAND, BY_HAND, BY_HAND)

void orma_emit_rcl_init(const orma_ev_rcl_init * ev)
{
  lttng_ust_tracepoint(orma, rcl_init, WORDS(ev));
  ROS2(rcl_init, PTR(ev->context), ORMA_TRACETOOLS_VERSION);
}

void orma_emit_rcl_node_init(
  const orma_ev_rcl_node_init * ev, const char * node_name, const char * node_namespace)
{
  lttng_ust_tracepoint(orma, rcl_node_init, WORDS(ev), node_name, node_namespace);
  ROS2(rcl_node_init, PTR(ev->node), PTR(ev->rmw_node), node_name, node_namespace);
}

void orma_emit_rmw_publisher_init(const orma_ev_rmw_endpoint_init * ev)
{
  lttng_ust_tracepoint(orma, rmw_publisher_init, WORDS(ev));
  ROS2(rmw_publisher_init, PTR(ev->rmw_handle), ev->gid);
}

void orma_emit_rcl_publisher_init(const orma_ev_rcl_topic_init * ev, const char * topic_name)
{
  lttng_ust_tracepoint(orma, rcl_publisher_init, WORDS(ev), topic_name);
  ROS2(
    rcl_publisher_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), topic_name,
    ev->queue_depth);
}

void orma_emit_rclcpp_publish(const orma_ev_publish * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_publish, WORDS(ev));
  ROS2(rclcpp_publish, PTR(ev->message));
}

void orma_emit_rcl_publish(const orma_ev_publish * ev)
{
  lttng_ust_tracepoint(orma, rcl_publish, WORDS(ev));
  ROS2(rcl_publish, PTR(ev->publisher), PTR(ev->message));
}

void orma_emit_rmw_publish_rcl(const orma_ev_rmw_publish_rcl * ev)
{
  lttng_ust_tracepoint(orma, rmw_publish_rcl, WORDS(ev));
  ROS2(rmw_publish, PTR(ev->message));
}

void orma_emit_rmw_subscription_init(const orma_ev_rmw_endpoint_init * ev)
{
  lttng_ust_tracepoint(orma, rmw_subscription_init, WORDS(ev));
  ROS2(rmw_subscription_init, PTR(ev->rmw_handle), ev->gid);
}

void orma_emit_rcl_subscription_init(const orma_ev_rcl_topic_init * ev, const char * topic_name)
{
  lttng_ust_tracepoint(orma, rcl_subscription_init, WORDS(ev), topic_name);
  ROS2(
    rcl_subscription_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), topic_name,
    ev->queue_depth);
}

void orma_emit_rclcpp_subscription_init(const orma_ev_rclcpp_subscription_init * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_subscription_init, WORDS(ev));
  ROS2(rclcpp_subscription_init, PTR(ev->subscription_handle), PTR(ev->subscription));
}

void orma_emit_rclcpp_subscription_callback_added(const orma_ev_callback_added * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_subscription_callback_added, WORDS(ev));
  ROS2(rclcpp_subscription_callback_added, PTR(ev->handle), PTR(ev->callback));
}

void orma_emit_rmw_take(const orma_ev_rmw_take * ev)
{
  lttng_ust_tracepoint(orma, rmw_take, WORDS(ev));
  ROS2(rmw_take, PTR(ev->rmw_subscription), PTR(ev->message), ev->source_timestamp, ev->taken);
}

void orma_emit_rcl_take(const orma_ev_take * ev)
{
  lttng_ust_tracepoint(orma, rcl_take, WORDS(ev));
  ROS2(rcl_take, PTR(ev->message));
}

void orma_emit_rclcpp_take(const orma_ev_take * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_take, WORDS(ev));
  ROS2(rclcpp_take, PTR(ev->message));
}

void orma_emit_rcl_service_init(const orma_ev_rcl_service_init * ev, const char * service_name)
{
  lttng_ust_tracepoint(orma, rcl_service_init, WORDS(ev), service_name);
  ROS2(rcl_service_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), service_name);
}

void orma_emit_rclcpp_service_callback_added(const orma_ev_callback_added * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_service_callback_added, WORDS(ev));
  ROS2(rclcpp_service_callback_added, PTR(ev->handle), PTR(ev->callback));
}

void orma_emit_rcl_client_init(const orma_ev_rcl_service_init * ev, const char * service_name)
{
  lttng_ust_tracepoint(orma, rcl_client_init, WORDS(ev), service_name);
  ROS2(rcl_client_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), service_name);
}

void orma_emit_rcl_timer_init(const orma_ev_rcl_timer_init * ev)
{
  lttng_ust_tracepoint(orma, rcl_timer_init, WORDS(ev));
  ROS2(rcl_timer_init, PTR(ev->timer), ev->period);
}

void orma_emit_rclcpp_timer_callback_added(const orma_ev_callback_added * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_timer_callback_added, WORDS(ev));
  ROS2(rclcpp_timer_callback_added, PTR(ev->handle), PTR(ev->callback));
}

void orma_emit_rclcpp_timer_link_node(const orma_ev_rclcpp_timer_link_node * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_timer_link_node, WORDS(ev));
  ROS2(rclcpp_timer_link_node, PTR(ev->timer), PTR(ev->node));
}

void orma_emit_rclcpp_callback_register(
  const orma_ev_rclcpp_callback_register * ev, const char * function_symbol)
{
  lttng_ust_tracepoint(orma, rclcpp_callback_register, WORDS(ev), function_symbol);
  ROS2(rclcpp_callback_register, PTR(ev->callback), function_symbol);
}

void orma_emit_callback_start(const orma_ev_callback_start * ev)
{
  lttng_ust_tracepoint(orma, callback_start, WORDS(ev));
  ROS2(callback_start, PTR(ev->callback), ev->is_intra_process);
  entered(ORMA_SRC_ROS, ev->callback, 0);
}

void orma_emit_callback_end(const orma_ev_handle * ev)
{
  leaving(ORMA_SRC_ROS, ev->handle, 0);
  lttng_ust_tracepoint(orma, callback_end, WORDS(ev));
  ROS2(callback_end, PTR(ev->handle));
}

void orma_emit_rclcpp_executor_get_next_ready(void)
{
  static const uint64_t none;
  lttng_ust_tracepoint(orma, rclcpp_executor_get_next_ready, &none, 0);
  ROS2(rclcpp_executor_get_next_ready);
}

void orma_emit_rclcpp_executor_wait_for_work(const orma_ev_rclcpp_executor_wait_for_work * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_executor_wait_for_work, WORDS(ev));
  ROS2(rclcpp_executor_wait_for_work, ev->timeout);
}

void orma_emit_rclcpp_executor_execute(const orma_ev_handle * ev)
{
  lttng_ust_tracepoint(orma, rclcpp_executor_execute, WORDS(ev));
  ROS2(rclcpp_executor_execute, PTR(ev->handle));
}

void orma_emit_flux_channel_init(const orma_ev_flux_channel_init * ev, const char * segment)
{
  lttng_ust_tracepoint(
    flux, channel_init, PTR(ev->channel), ev->dev, ev->ino, segment, ev->publisher, ev->slot_count,
    ev->slot_size, ev->stamp_offset);
}

void orma_emit_flux_claim(const orma_ev_flux_claim * ev)
{
  lttng_ust_tracepoint(flux, claim, PTR(ev->channel), ev->slot, ev->result, ev->loan);
}

void orma_emit_flux_loan(const orma_ev_flux_loan * ev)
{
  lttng_ust_tracepoint(flux, loan, PTR(ev->channel), ev->slot);
}

void orma_emit_flux_loan_end(const orma_ev_flux_loan_end * ev)
{
  lttng_ust_tracepoint(flux, loan_end, PTR(ev->channel), ev->slot, ev->result);
}

void orma_emit_flux_commit(const orma_ev_flux_commit * ev)
{
  lttng_ust_tracepoint(
    flux, commit, PTR(ev->channel), ev->ticket, ev->slot, ev->nbytes, ev->woke, ev->stamp);
}

void orma_emit_flux_take(const orma_ev_flux_take * ev)
{
  lttng_ust_tracepoint(flux, take, PTR(ev->channel), ev->ticket, ev->slot, ev->lost, ev->stamp);
}

void orma_emit_flux_release(const orma_ev_flux_release * ev)
{
  lttng_ust_tracepoint(flux, release, PTR(ev->channel), ev->ticket, ev->held);
}

void orma_emit_flux_refused(const orma_ev_flux_refused * ev)
{
  lttng_ust_tracepoint(flux, refused, PTR(ev->channel), ev->reason);
}

void orma_emit_flux_wait(const orma_ev_flux_wait * ev)
{
  lttng_ust_tracepoint(flux, wait, PTR(ev->executor), ev->sources, ev->timeout_ns);
}

void orma_emit_flux_wake(const orma_ev_flux_wake * ev)
{
  lttng_ust_tracepoint(flux, wake, PTR(ev->executor), ev->result, ev->entry);
}

void orma_emit_flux_callback_start(const orma_ev_flux_callback_start * ev)
{
  lttng_ust_tracepoint(flux, callback_start, PTR(ev->executor), ev->entry, PTR(ev->endpoint));
  entered(ORMA_SRC_FLUX, ev->executor, ev->entry);
}

void orma_emit_flux_callback_end(const orma_ev_flux_callback_end * ev, const char * segment)
{
  leaving(ORMA_SRC_FLUX, ev->executor, ev->entry);
  lttng_ust_tracepoint(
    flux, callback_end, PTR(ev->executor), ev->entry, ev->delivered, PTR(ev->channel), segment);
}

void orma_emit_flux_endpoint_init(
  const orma_ev_flux_endpoint_init * ev, const char * node, const char * key, const char * signpost)
{
  lttng_ust_tracepoint(
    flux, endpoint_init, PTR(ev->endpoint), node, key, signpost, ev->fingerprint, ev->role,
    ev->lang);
}

void orma_emit_flux_py_gil(const orma_ev_flux_py_gil * ev)
{
  lttng_ust_tracepoint(flux, py_gil, ev->phase);
}

void orma_emit_flux_py_ready(const orma_ev_flux_py_ready * ev)
{
  lttng_ust_tracepoint(flux, py_ready, PTR(ev->channel), ev->ticket);
}

void orma_emit_flux_record_drop(const orma_ev_flux_record_drop * ev)
{
  lttng_ust_tracepoint(flux, record_drop, PTR(ev->channel), ev->ticket, ev->count, ev->reason);
}

void orma_emit_flux_gpu_wedged(const orma_ev_flux_gpu_wedged * ev)
{
  lttng_ust_tracepoint(flux, gpu_wedged, ev->result);
}

void orma_backend_missing(uint16_t event)
{
  lttng_ust_tracepoint(orma, missing, event);
}

bool orma_backend_malloc_on(void)
{
  return false;
}

// Asked for in this library's own file, which appears on the first callback.
bool orma_backend_gpu_on(void)
{
  return orma_option(ORMA_OPTION_GPU);
}

bool orma_backend_dds_on(void)
{
  return orma_option(ORMA_OPTION_DDS);
}

void orma_backend_alloc(size_t bytes)
{
  (void)bytes;
}

void orma_backend_free(size_t bytes)
{
  (void)bytes;
}
