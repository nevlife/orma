// ros_trace_* of libtracetools.so (design 2.1), and their gated enabled_/do_ forms (2.4). Each
// emits its event, then calls the next definition. The installed tracetools.h declares them, so a
// signature that differs from this distro's fails the build. orma_tracepoints.h is generated from
// the library's exports: ORMA_TP_<name> for each, ORMA_TP_GATES when it has the gated forms.
#define _GNU_SOURCE
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"
#include "orma_preload/orma_tracepoints.h"
#include "tracetools/tracetools.h"

#include <stdbool.h>
#include <sys/uio.h>
#include <unistd.h>

#define TRACETOOLS "libtracetools.so"

_Static_assert(ORMA_RMW_GID_SIZE <= ORMA_GID_MAX, "rmw gid");

#if ORMA_TP_GATES
// enabled_ answers true so that the caller builds the arguments and calls do_.
#define ROS_GATE(name, event, params, args)                                 \
  ORMA_EXPORT bool ros_trace_enabled_##name(void)                           \
  {                                                                         \
    return true;                                                            \
  }                                                                         \
  ORMA_EXPORT void ros_trace_do_##name params                               \
  {                                                                         \
    emit_##name args;                                                       \
    void(*next) params = ORMA_NEXT(ros_trace_do_##name, TRACETOOLS, event); \
    if (next != NULL) next args;                                            \
  }
#else
#define ROS_GATE(name, event, params, args)
#endif

#define ROS_POINT(name, event, params, args)                             \
  static void emit_##name params;                                        \
  ORMA_EXPORT void ros_trace_##name params                               \
  {                                                                      \
    emit_##name args;                                                    \
    void(*next) params = ORMA_NEXT(ros_trace_##name, TRACETOOLS, event); \
    if (next != NULL) next args;                                         \
  }                                                                      \
  ROS_GATE(name, event, params, args)                                    \
  static void emit_##name params

static void endpoint(orma_ev_rmw_endpoint_init * ev, const void * handle, const uint8_t * gid)
{
  ev->rmw_handle = ORMA_U64(handle);
  memcpy(ev->gid, gid, ORMA_RMW_GID_SIZE);
  ev->gid_len = ORMA_RMW_GID_SIZE;
}

// The words at the callback handle's offsets 8 to 40 hold the std::function's code address when
// there is one (design 2.4). The handle of a small timer functor can end before that, so the read
// goes through the kernel and an unreadable word stays 0.
static void callback_code(const void * callback)
{
  uint64_t words[5] = {0};
  struct iovec local = {words, sizeof words};
  struct iovec remote = {(char *)callback + 8, sizeof words};
  (void)process_vm_readv(getpid(), &local, 1, &remote, 1, 0);
  orma_ev_callback_code ev = {
    .callback = ORMA_U64(callback),
    .words = {words[2], words[1], words[3], words[4], words[0]},
  };
  orma_emit_callback_code(&ev);
}

static void callback_added(
  void (*emit)(const orma_ev_callback_added *), const void * handle, const void * callback)
{
  orma_ev_callback_added ev = {.handle = ORMA_U64(handle), .callback = ORMA_U64(callback)};
  emit(&ev);
  callback_code(callback);
}

static void publish(
  void (*emit)(const orma_ev_publish *), const void * publisher, const void * message)
{
  orma_ev_publish ev = {
    .publisher = ORMA_U64(publisher), .message = ORMA_U64(message), .stamp = orma_stamp(message)};
  emit(&ev);
}

ROS_POINT(rcl_init, ORMA_EV_RCL_INIT, (const void * context_handle), (context_handle))
{
  orma_ev_rcl_init ev = {.context = ORMA_U64(context_handle)};
  orma_emit_rcl_init(&ev);
}

ROS_POINT(
  rcl_node_init, ORMA_EV_RCL_NODE_INIT,
  (const void * node_handle, const void * rmw_handle, const char * node_name,
   const char * node_namespace),
  (node_handle, rmw_handle, node_name, node_namespace))
{
  orma_ev_rcl_node_init ev = {.node = ORMA_U64(node_handle), .rmw_node = ORMA_U64(rmw_handle)};
  orma_emit_rcl_node_init(&ev, node_name, node_namespace);
}

ROS_POINT(
  rmw_publisher_init, ORMA_EV_RMW_PUBLISHER_INIT,
  (const void * rmw_publisher_handle, const uint8_t * gid), (rmw_publisher_handle, gid))
{
  orma_ev_rmw_endpoint_init ev = {0};
  endpoint(&ev, rmw_publisher_handle, gid);
  orma_emit_rmw_publisher_init(&ev);
}

ROS_POINT(
  rcl_publisher_init, ORMA_EV_RCL_PUBLISHER_INIT,
  (const void * publisher_handle, const void * node_handle, const void * rmw_publisher_handle,
   const char * topic_name, const size_t queue_depth),
  (publisher_handle, node_handle, rmw_publisher_handle, topic_name, queue_depth))
{
  orma_ev_rcl_topic_init ev = {
    .handle = ORMA_U64(publisher_handle),
    .node = ORMA_U64(node_handle),
    .rmw_handle = ORMA_U64(rmw_publisher_handle),
    .queue_depth = queue_depth,
  };
  orma_emit_rcl_publisher_init(&ev, topic_name);
}

ROS_POINT(
  rclcpp_publish, ORMA_EV_RCLCPP_PUBLISH, (const void * publisher_handle, const void * message),
  (publisher_handle, message))
{
  publish(orma_emit_rclcpp_publish, publisher_handle, message);
}

#if ORMA_TP_rclcpp_intra_publish
ROS_POINT(
  rclcpp_intra_publish, ORMA_EV_RCLCPP_INTRA_PUBLISH,
  (const void * publisher_handle, const void * message), (publisher_handle, message))
{
  publish(orma_emit_rclcpp_intra_publish, publisher_handle, message);
}
#endif

ROS_POINT(
  rcl_publish, ORMA_EV_RCL_PUBLISH, (const void * publisher_handle, const void * message),
  (publisher_handle, message))
{
  publish(orma_emit_rcl_publish, publisher_handle, message);
}

#if ORMA_TRACETOOLS_MAJOR >= 8
ROS_POINT(
  rmw_publish, ORMA_EV_RMW_PUBLISH,
  (const void * rmw_publisher_handle, const void * message, int64_t timestamp),
  (rmw_publisher_handle, message, timestamp))
{
  orma_ev_rmw_publish ev = {
    .rmw_publisher = ORMA_U64(rmw_publisher_handle),
    .message = ORMA_U64(message),
    .timestamp = timestamp,
    .stamp = orma_stamp(message),
  };
  orma_emit_rmw_publish(&ev);
}
#else
ROS_POINT(rmw_publish, ORMA_EV_RMW_PUBLISH_RCL, (const void * message), (message))
{
  orma_ev_rmw_publish_rcl ev = {.message = ORMA_U64(message), .stamp = orma_stamp(message)};
  orma_emit_rmw_publish_rcl(&ev);
}
#endif

ROS_POINT(
  rmw_subscription_init, ORMA_EV_RMW_SUBSCRIPTION_INIT,
  (const void * rmw_subscription_handle, const uint8_t * gid), (rmw_subscription_handle, gid))
{
  orma_ev_rmw_endpoint_init ev = {0};
  endpoint(&ev, rmw_subscription_handle, gid);
  orma_emit_rmw_subscription_init(&ev);
}

ROS_POINT(
  rcl_subscription_init, ORMA_EV_RCL_SUBSCRIPTION_INIT,
  (const void * subscription_handle, const void * node_handle, const void * rmw_subscription_handle,
   const char * topic_name, const size_t queue_depth),
  (subscription_handle, node_handle, rmw_subscription_handle, topic_name, queue_depth))
{
  orma_ev_rcl_topic_init ev = {
    .handle = ORMA_U64(subscription_handle),
    .node = ORMA_U64(node_handle),
    .rmw_handle = ORMA_U64(rmw_subscription_handle),
    .queue_depth = queue_depth,
  };
  orma_emit_rcl_subscription_init(&ev, topic_name);
}

ROS_POINT(
  rclcpp_subscription_init, ORMA_EV_RCLCPP_SUBSCRIPTION_INIT,
  (const void * subscription_handle, const void * subscription),
  (subscription_handle, subscription))
{
  orma_ev_rclcpp_subscription_init ev = {
    .subscription_handle = ORMA_U64(subscription_handle),
    .subscription = ORMA_U64(subscription),
  };
  orma_emit_rclcpp_subscription_init(&ev);
}

ROS_POINT(
  rclcpp_subscription_callback_added, ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
  (const void * subscription, const void * callback), (subscription, callback))
{
  callback_added(orma_emit_rclcpp_subscription_callback_added, subscription, callback);
}

ROS_POINT(
  rmw_take, ORMA_EV_RMW_TAKE,
  (const void * rmw_subscription_handle, const void * message, int64_t source_timestamp,
   const bool taken),
  (rmw_subscription_handle, message, source_timestamp, taken))
{
  orma_ev_rmw_take ev = {
    .rmw_subscription = ORMA_U64(rmw_subscription_handle),
    .message = ORMA_U64(message),
    .source_timestamp = source_timestamp,
    .stamp = taken ? orma_stamp(message) : 0,
    .taken = taken,
  };
  orma_emit_rmw_take(&ev);
}

ROS_POINT(rcl_take, ORMA_EV_RCL_TAKE, (const void * message), (message))
{
  orma_ev_take ev = {.message = ORMA_U64(message)};
  orma_emit_rcl_take(&ev);
}

ROS_POINT(rclcpp_take, ORMA_EV_RCLCPP_TAKE, (const void * message), (message))
{
  orma_ev_take ev = {.message = ORMA_U64(message)};
  orma_emit_rclcpp_take(&ev);
}

ROS_POINT(
  rcl_service_init, ORMA_EV_RCL_SERVICE_INIT,
  (const void * service_handle, const void * node_handle, const void * rmw_service_handle,
   const char * service_name),
  (service_handle, node_handle, rmw_service_handle, service_name))
{
  orma_ev_rcl_service_init ev = {
    .handle = ORMA_U64(service_handle),
    .node = ORMA_U64(node_handle),
    .rmw_handle = ORMA_U64(rmw_service_handle),
  };
  orma_emit_rcl_service_init(&ev, service_name);
}

ROS_POINT(
  rclcpp_service_callback_added, ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED,
  (const void * service_handle, const void * callback), (service_handle, callback))
{
  callback_added(orma_emit_rclcpp_service_callback_added, service_handle, callback);
}

ROS_POINT(
  rcl_client_init, ORMA_EV_RCL_CLIENT_INIT,
  (const void * client_handle, const void * node_handle, const void * rmw_client_handle,
   const char * service_name),
  (client_handle, node_handle, rmw_client_handle, service_name))
{
  orma_ev_rcl_service_init ev = {
    .handle = ORMA_U64(client_handle),
    .node = ORMA_U64(node_handle),
    .rmw_handle = ORMA_U64(rmw_client_handle),
  };
  orma_emit_rcl_client_init(&ev, service_name);
}

ROS_POINT(
  rcl_timer_init, ORMA_EV_RCL_TIMER_INIT, (const void * timer_handle, int64_t period),
  (timer_handle, period))
{
  orma_ev_rcl_timer_init ev = {.timer = ORMA_U64(timer_handle), .period = period};
  orma_emit_rcl_timer_init(&ev);
}

ROS_POINT(
  rclcpp_timer_callback_added, ORMA_EV_RCLCPP_TIMER_CALLBACK_ADDED,
  (const void * timer_handle, const void * callback), (timer_handle, callback))
{
  callback_added(orma_emit_rclcpp_timer_callback_added, timer_handle, callback);
}

ROS_POINT(
  rclcpp_timer_link_node, ORMA_EV_RCLCPP_TIMER_LINK_NODE,
  (const void * timer_handle, const void * node_handle), (timer_handle, node_handle))
{
  orma_ev_rclcpp_timer_link_node ev = {
    .timer = ORMA_U64(timer_handle), .node = ORMA_U64(node_handle)};
  orma_emit_rclcpp_timer_link_node(&ev);
}

ROS_POINT(
  rclcpp_callback_register, ORMA_EV_RCLCPP_CALLBACK_REGISTER,
  (const void * callback, const char * function_symbol), (callback, function_symbol))
{
  orma_ev_rclcpp_callback_register ev = {.callback = ORMA_U64(callback)};
  orma_emit_rclcpp_callback_register(&ev, function_symbol);
}

ROS_POINT(
  callback_start, ORMA_EV_CALLBACK_START, (const void * callback, const bool is_intra_process),
  (callback, is_intra_process))
{
  orma_ev_callback_start ev = {
    .callback = ORMA_U64(callback), .is_intra_process = is_intra_process};
  orma_emit_callback_start(&ev);
}

ROS_POINT(callback_end, ORMA_EV_CALLBACK_END, (const void * callback), (callback))
{
  orma_ev_handle ev = {.handle = ORMA_U64(callback)};
  orma_emit_callback_end(&ev);
}

ROS_POINT(rclcpp_executor_get_next_ready, ORMA_EV_RCLCPP_EXECUTOR_GET_NEXT_READY, (void), ())
{
  orma_emit_rclcpp_executor_get_next_ready();
}

ROS_POINT(
  rclcpp_executor_wait_for_work, ORMA_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK, (const int64_t timeout),
  (timeout))
{
  orma_ev_rclcpp_executor_wait_for_work ev = {.timeout = timeout};
  orma_emit_rclcpp_executor_wait_for_work(&ev);
}

ROS_POINT(rclcpp_executor_execute, ORMA_EV_RCLCPP_EXECUTOR_EXECUTE, (const void * handle), (handle))
{
  orma_ev_handle ev = {.handle = ORMA_U64(handle)};
  orma_emit_rclcpp_executor_execute(&ev);
}
