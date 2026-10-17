// The backend of libarmo_lttng.so (design 3, 13.7): each armo_emit_<name> becomes one LTTng event,
// of provider armo, or of provider flux2 with the flux2 function's arguments as fields. Where
// libtracetools has no LTTng (Humble), the ROS events also go out as provider ros2 events in
// tracetools' own layout. The clock and the aggregation are armo-collectd's. The one per-thread
// state is the current callback, in a backend 2 file whose slots hold no table and no ring, for
// armod's off-CPU attribution (design 13.2, 15.2).
// The tracepoint definitions of the three providers, once for the library.
#define LTTNG_UST_TRACEPOINT_DEFINE
#include "armo_lttng/armo_tp.h"
#include "armo_lttng/flux2_tp.h"
#include "armo_preload/armo_emit.h"
#include "armo_preload/slots.h"
#if ARMO_ROS2_PROVIDER
#include "armo_lttng/ros2_tp.h"
#define ROS2(...) lttng_ust_tracepoint(ros2, __VA_ARGS__)
#else
#define ROS2(...) ((void)0)
#endif

#define WORDS(ev) (const uint64_t *)(ev), (uint64_t)(sizeof *(ev) / sizeof(uint64_t))
#define PTR(v) ((const void *)(uintptr_t)(v))

// Its own name, apart from libarmo_preload.so's file when a process loads both.
const struct armo_layout armo_layout = {.backend = ARMO_BACKEND_LTTNG, .suffix = ".lttng"};

// The current callback is set after the start event and cleared before the end event, so the
// off-CPU time armod charges to it lies within the callback's time.
static inline __attribute__((always_inline)) void entered(
  uint16_t src, uint64_t key, uint32_t entry)
{
  struct armo_thread * t = armo_thread();
  if (t != NULL) armo_push(t, src, key, entry, 0);
}

static inline __attribute__((always_inline)) void leaving(
  uint16_t src, uint64_t key, uint32_t entry)
{
  struct armo_thread * t = armo_thread();
  uint64_t t0;
  if (t != NULL) armo_pop(t, src, key, entry, &t0);
}

#define CHECK(name, type) \
  _Static_assert(sizeof(type) % sizeof(uint64_t) == 0, #type " is not whole words");
#define CHECK1(name, type, a) CHECK(name, type)
#define CHECK2(name, type, a, b) CHECK(name, type)
ARMO_LTTNG_EVENTS(CHECK, CHECK1, CHECK2, CHECK, CHECK1, CHECK2)

#define EMIT0(name, type)                        \
  void armo_emit_##name(const type * ev)         \
  {                                              \
    lttng_ust_tracepoint(armo, name, WORDS(ev)); \
  }
#define EMIT1(name, type, a)                             \
  void armo_emit_##name(const type * ev, const char * a) \
  {                                                      \
    lttng_ust_tracepoint(armo, name, WORDS(ev), a);      \
  }
#define EMIT2(name, type, a, b)                                          \
  void armo_emit_##name(const type * ev, const char * a, const char * b) \
  {                                                                      \
    lttng_ust_tracepoint(armo, name, WORDS(ev), a, b);                   \
  }
#define BY_HAND(...)
ARMO_LTTNG_EVENTS(EMIT0, EMIT1, EMIT2, BY_HAND, BY_HAND, BY_HAND)

void armo_emit_rcl_init(const armo_ev_rcl_init * ev)
{
  lttng_ust_tracepoint(armo, rcl_init, WORDS(ev));
  ROS2(rcl_init, PTR(ev->context), ARMO_TRACETOOLS_VERSION);
}

void armo_emit_rcl_node_init(
  const armo_ev_rcl_node_init * ev, const char * node_name, const char * node_namespace)
{
  lttng_ust_tracepoint(armo, rcl_node_init, WORDS(ev), node_name, node_namespace);
  ROS2(rcl_node_init, PTR(ev->node), PTR(ev->rmw_node), node_name, node_namespace);
}

void armo_emit_rmw_publisher_init(const armo_ev_rmw_endpoint_init * ev)
{
  lttng_ust_tracepoint(armo, rmw_publisher_init, WORDS(ev));
  ROS2(rmw_publisher_init, PTR(ev->rmw_handle), ev->gid);
}

void armo_emit_rcl_publisher_init(const armo_ev_rcl_topic_init * ev, const char * topic_name)
{
  lttng_ust_tracepoint(armo, rcl_publisher_init, WORDS(ev), topic_name);
  ROS2(
    rcl_publisher_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), topic_name,
    ev->queue_depth);
}

void armo_emit_rclcpp_publish(const armo_ev_publish * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_publish, WORDS(ev));
  ROS2(rclcpp_publish, PTR(ev->message));
}

void armo_emit_rcl_publish(const armo_ev_publish * ev)
{
  lttng_ust_tracepoint(armo, rcl_publish, WORDS(ev));
  ROS2(rcl_publish, PTR(ev->publisher), PTR(ev->message));
}

void armo_emit_rmw_publish_rcl(const armo_ev_rmw_publish_rcl * ev)
{
  lttng_ust_tracepoint(armo, rmw_publish_rcl, WORDS(ev));
  ROS2(rmw_publish, PTR(ev->message));
}

void armo_emit_rmw_subscription_init(const armo_ev_rmw_endpoint_init * ev)
{
  lttng_ust_tracepoint(armo, rmw_subscription_init, WORDS(ev));
  ROS2(rmw_subscription_init, PTR(ev->rmw_handle), ev->gid);
}

void armo_emit_rcl_subscription_init(const armo_ev_rcl_topic_init * ev, const char * topic_name)
{
  lttng_ust_tracepoint(armo, rcl_subscription_init, WORDS(ev), topic_name);
  ROS2(
    rcl_subscription_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), topic_name,
    ev->queue_depth);
}

void armo_emit_rclcpp_subscription_init(const armo_ev_rclcpp_subscription_init * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_subscription_init, WORDS(ev));
  ROS2(rclcpp_subscription_init, PTR(ev->subscription_handle), PTR(ev->subscription));
}

void armo_emit_rclcpp_subscription_callback_added(const armo_ev_callback_added * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_subscription_callback_added, WORDS(ev));
  ROS2(rclcpp_subscription_callback_added, PTR(ev->handle), PTR(ev->callback));
}

void armo_emit_rmw_take(const armo_ev_rmw_take * ev)
{
  lttng_ust_tracepoint(armo, rmw_take, WORDS(ev));
  ROS2(rmw_take, PTR(ev->rmw_subscription), PTR(ev->message), ev->source_timestamp, ev->taken);
}

void armo_emit_rcl_take(const armo_ev_take * ev)
{
  lttng_ust_tracepoint(armo, rcl_take, WORDS(ev));
  ROS2(rcl_take, PTR(ev->message));
}

void armo_emit_rclcpp_take(const armo_ev_take * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_take, WORDS(ev));
  ROS2(rclcpp_take, PTR(ev->message));
}

void armo_emit_rcl_service_init(const armo_ev_rcl_service_init * ev, const char * service_name)
{
  lttng_ust_tracepoint(armo, rcl_service_init, WORDS(ev), service_name);
  ROS2(rcl_service_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), service_name);
}

void armo_emit_rclcpp_service_callback_added(const armo_ev_callback_added * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_service_callback_added, WORDS(ev));
  ROS2(rclcpp_service_callback_added, PTR(ev->handle), PTR(ev->callback));
}

void armo_emit_rcl_client_init(const armo_ev_rcl_service_init * ev, const char * service_name)
{
  lttng_ust_tracepoint(armo, rcl_client_init, WORDS(ev), service_name);
  ROS2(rcl_client_init, PTR(ev->handle), PTR(ev->node), PTR(ev->rmw_handle), service_name);
}

void armo_emit_rcl_timer_init(const armo_ev_rcl_timer_init * ev)
{
  lttng_ust_tracepoint(armo, rcl_timer_init, WORDS(ev));
  ROS2(rcl_timer_init, PTR(ev->timer), ev->period);
}

void armo_emit_rclcpp_timer_callback_added(const armo_ev_callback_added * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_timer_callback_added, WORDS(ev));
  ROS2(rclcpp_timer_callback_added, PTR(ev->handle), PTR(ev->callback));
}

void armo_emit_rclcpp_timer_link_node(const armo_ev_rclcpp_timer_link_node * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_timer_link_node, WORDS(ev));
  ROS2(rclcpp_timer_link_node, PTR(ev->timer), PTR(ev->node));
}

void armo_emit_rclcpp_callback_register(
  const armo_ev_rclcpp_callback_register * ev, const char * function_symbol)
{
  lttng_ust_tracepoint(armo, rclcpp_callback_register, WORDS(ev), function_symbol);
  ROS2(rclcpp_callback_register, PTR(ev->callback), function_symbol);
}

void armo_emit_callback_start(const armo_ev_callback_start * ev)
{
  lttng_ust_tracepoint(armo, callback_start, WORDS(ev));
  ROS2(callback_start, PTR(ev->callback), ev->is_intra_process);
  entered(ARMO_SRC_ROS, ev->callback, 0);
}

void armo_emit_callback_end(const armo_ev_handle * ev)
{
  leaving(ARMO_SRC_ROS, ev->handle, 0);
  lttng_ust_tracepoint(armo, callback_end, WORDS(ev));
  ROS2(callback_end, PTR(ev->handle));
}

void armo_emit_rclcpp_executor_get_next_ready(void)
{
  static const uint64_t none;
  lttng_ust_tracepoint(armo, rclcpp_executor_get_next_ready, &none, 0);
  ROS2(rclcpp_executor_get_next_ready);
}

void armo_emit_rclcpp_executor_wait_for_work(const armo_ev_rclcpp_executor_wait_for_work * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_executor_wait_for_work, WORDS(ev));
  ROS2(rclcpp_executor_wait_for_work, ev->timeout);
}

void armo_emit_rclcpp_executor_execute(const armo_ev_handle * ev)
{
  lttng_ust_tracepoint(armo, rclcpp_executor_execute, WORDS(ev));
  ROS2(rclcpp_executor_execute, PTR(ev->handle));
}

void armo_emit_flux2_channel_init(const armo_ev_flux2_channel_init * ev, const char * segment)
{
  lttng_ust_tracepoint(
    flux2, channel_init, PTR(ev->channel), ev->dev, ev->ino, segment, ev->publisher, ev->slot_count,
    ev->slot_size, ev->stamp_offset);
}

void armo_emit_flux2_claim(const armo_ev_flux2_claim * ev)
{
  lttng_ust_tracepoint(flux2, claim, PTR(ev->channel), ev->slot, ev->result, ev->loan);
}

void armo_emit_flux2_loan(const armo_ev_flux2_loan * ev)
{
  lttng_ust_tracepoint(flux2, loan, PTR(ev->channel), ev->slot);
}

void armo_emit_flux2_loan_end(const armo_ev_flux2_loan_end * ev)
{
  lttng_ust_tracepoint(flux2, loan_end, PTR(ev->channel), ev->slot, ev->result);
}

void armo_emit_flux2_commit(const armo_ev_flux2_commit * ev)
{
  lttng_ust_tracepoint(
    flux2, commit, PTR(ev->channel), ev->ticket, ev->slot, ev->nbytes, ev->woke, ev->stamp);
}

void armo_emit_flux2_take(const armo_ev_flux2_take * ev)
{
  lttng_ust_tracepoint(flux2, take, PTR(ev->channel), ev->ticket, ev->slot, ev->lost, ev->stamp);
}

void armo_emit_flux2_release(const armo_ev_flux2_release * ev)
{
  lttng_ust_tracepoint(flux2, release, PTR(ev->channel), ev->ticket, ev->held);
}

void armo_emit_flux2_refused(const armo_ev_flux2_refused * ev)
{
  lttng_ust_tracepoint(flux2, refused, PTR(ev->channel), ev->reason);
}

void armo_emit_flux2_wait(const armo_ev_flux2_wait * ev)
{
  lttng_ust_tracepoint(flux2, wait, PTR(ev->executor), ev->sources, ev->timeout_ns);
}

void armo_emit_flux2_wake(const armo_ev_flux2_wake * ev)
{
  lttng_ust_tracepoint(flux2, wake, PTR(ev->executor), ev->result, ev->entry);
}

void armo_emit_flux2_callback_start(const armo_ev_flux2_callback_start * ev)
{
  lttng_ust_tracepoint(flux2, callback_start, PTR(ev->executor), ev->entry, PTR(ev->endpoint));
  entered(ARMO_SRC_FLUX2, ev->executor, ev->entry);
}

void armo_emit_flux2_callback_end(const armo_ev_flux2_callback_end * ev, const char * segment)
{
  leaving(ARMO_SRC_FLUX2, ev->executor, ev->entry);
  lttng_ust_tracepoint(
    flux2, callback_end, PTR(ev->executor), ev->entry, ev->delivered, PTR(ev->channel), segment);
}

void armo_emit_flux2_endpoint_init(
  const armo_ev_flux2_endpoint_init * ev, const char * node, const char * key,
  const char * signpost)
{
  lttng_ust_tracepoint(
    flux2, endpoint_init, PTR(ev->endpoint), node, key, signpost, ev->fingerprint, ev->role,
    ev->lang);
}

void armo_emit_flux2_py_gil(const armo_ev_flux2_py_gil * ev)
{
  lttng_ust_tracepoint(flux2, py_gil, ev->phase);
}

void armo_emit_flux2_py_ready(const armo_ev_flux2_py_ready * ev)
{
  lttng_ust_tracepoint(flux2, py_ready, PTR(ev->channel), ev->ticket);
}

void armo_emit_flux2_record_drop(const armo_ev_flux2_record_drop * ev)
{
  lttng_ust_tracepoint(flux2, record_drop, PTR(ev->channel), ev->ticket, ev->count, ev->reason);
}

void armo_emit_flux2_gpu_wedged(const armo_ev_flux2_gpu_wedged * ev)
{
  lttng_ust_tracepoint(flux2, gpu_wedged, ev->result);
}

void armo_backend_missing(uint16_t event)
{
  lttng_ust_tracepoint(armo, missing, event);
}

bool armo_backend_malloc_on(void)
{
  return false;
}

// Asked for in this library's own file, which appears on the first callback.
bool armo_backend_gpu_on(void)
{
  return armo_option(ARMO_OPTION_GPU);
}

bool armo_backend_dds_on(void)
{
  return armo_option(ARMO_OPTION_DDS);
}

void armo_backend_alloc(size_t bytes)
{
  (void)bytes;
}

void armo_backend_free(size_t bytes)
{
  (void)bytes;
}
