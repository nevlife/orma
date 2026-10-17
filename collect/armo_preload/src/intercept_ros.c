// ros_trace_* of libtracetools.so (design 2.1), and their gated enabled_/do_ forms (2.4). Each
// emits its event, then calls the next definition. The installed tracetools.h declares them, so a
// signature that differs from this distro's fails the build. armo_tracepoints.h is generated from
// the library's exports: ARMO_TP_<name> for each, ARMO_TP_GATES when it has the gated forms.
#define _GNU_SOURCE
#include "armo_preload/armo_emit.h"
#include "armo_preload/armo_tracepoints.h"
#include "armo_preload/intercept.h"
#include "tracetools/tracetools.h"

#include <stdbool.h>
#include <sys/uio.h>
#include <unistd.h>

#define TRACETOOLS "libtracetools.so"

_Static_assert(ARMO_RMW_GID_SIZE <= ARMO_GID_MAX, "rmw gid");

#if ARMO_TP_GATES
// enabled_ answers true so that the caller builds the arguments and calls do_.
#define ROS_GATE(name, event, params, args)                                 \
  ARMO_EXPORT bool ros_trace_enabled_##name(void)                           \
  {                                                                         \
    return true;                                                            \
  }                                                                         \
  ARMO_EXPORT void ros_trace_do_##name params                               \
  {                                                                         \
    emit_##name args;                                                       \
    void(*next) params = ARMO_NEXT(ros_trace_do_##name, TRACETOOLS, event); \
    if (next != NULL) next args;                                            \
  }
#else
#define ROS_GATE(name, event, params, args)
#endif

#define ROS_POINT(name, event, params, args)                             \
  static void emit_##name params;                                        \
  ARMO_EXPORT void ros_trace_##name params                               \
  {                                                                      \
    emit_##name args;                                                    \
    void(*next) params = ARMO_NEXT(ros_trace_##name, TRACETOOLS, event); \
    if (next != NULL) next args;                                         \
  }                                                                      \
  ROS_GATE(name, event, params, args)                                    \
  static void emit_##name params

static void endpoint(armo_ev_rmw_endpoint_init * ev, const void * handle, const uint8_t * gid)
{
  ev->rmw_handle = ARMO_U64(handle);
  memcpy(ev->gid, gid, ARMO_RMW_GID_SIZE);
  ev->gid_len = ARMO_RMW_GID_SIZE;
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
  armo_ev_callback_code ev = {
    .callback = ARMO_U64(callback),
    .words = {words[2], words[1], words[3], words[4], words[0]},
  };
  armo_emit_callback_code(&ev);
}

static void callback_added(
  void (*emit)(const armo_ev_callback_added *), const void * handle, const void * callback)
{
  armo_ev_callback_added ev = {.handle = ARMO_U64(handle), .callback = ARMO_U64(callback)};
  emit(&ev);
  callback_code(callback);
}

static void publish(
  void (*emit)(const armo_ev_publish *), const void * publisher, const void * message)
{
  armo_ev_publish ev = {
    .publisher = ARMO_U64(publisher), .message = ARMO_U64(message), .stamp = armo_stamp(message)};
  emit(&ev);
}

ROS_POINT(rcl_init, ARMO_EV_RCL_INIT, (const void * context_handle), (context_handle))
{
  armo_ev_rcl_init ev = {.context = ARMO_U64(context_handle)};
  armo_emit_rcl_init(&ev);
}

ROS_POINT(
  rcl_node_init, ARMO_EV_RCL_NODE_INIT,
  (const void * node_handle, const void * rmw_handle, const char * node_name,
   const char * node_namespace),
  (node_handle, rmw_handle, node_name, node_namespace))
{
  armo_ev_rcl_node_init ev = {.node = ARMO_U64(node_handle), .rmw_node = ARMO_U64(rmw_handle)};
  armo_emit_rcl_node_init(&ev, node_name, node_namespace);
}

ROS_POINT(
  rmw_publisher_init, ARMO_EV_RMW_PUBLISHER_INIT,
  (const void * rmw_publisher_handle, const uint8_t * gid), (rmw_publisher_handle, gid))
{
  armo_ev_rmw_endpoint_init ev = {0};
  endpoint(&ev, rmw_publisher_handle, gid);
  armo_emit_rmw_publisher_init(&ev);
}

ROS_POINT(
  rcl_publisher_init, ARMO_EV_RCL_PUBLISHER_INIT,
  (const void * publisher_handle, const void * node_handle, const void * rmw_publisher_handle,
   const char * topic_name, const size_t queue_depth),
  (publisher_handle, node_handle, rmw_publisher_handle, topic_name, queue_depth))
{
  armo_ev_rcl_topic_init ev = {
    .handle = ARMO_U64(publisher_handle),
    .node = ARMO_U64(node_handle),
    .rmw_handle = ARMO_U64(rmw_publisher_handle),
    .queue_depth = queue_depth,
  };
  armo_emit_rcl_publisher_init(&ev, topic_name);
}

ROS_POINT(
  rclcpp_publish, ARMO_EV_RCLCPP_PUBLISH, (const void * publisher_handle, const void * message),
  (publisher_handle, message))
{
  publish(armo_emit_rclcpp_publish, publisher_handle, message);
}

#if ARMO_TP_rclcpp_intra_publish
ROS_POINT(
  rclcpp_intra_publish, ARMO_EV_RCLCPP_INTRA_PUBLISH,
  (const void * publisher_handle, const void * message), (publisher_handle, message))
{
  publish(armo_emit_rclcpp_intra_publish, publisher_handle, message);
}
#endif

ROS_POINT(
  rcl_publish, ARMO_EV_RCL_PUBLISH, (const void * publisher_handle, const void * message),
  (publisher_handle, message))
{
  publish(armo_emit_rcl_publish, publisher_handle, message);
}

#if ARMO_TRACETOOLS_MAJOR >= 8
ROS_POINT(
  rmw_publish, ARMO_EV_RMW_PUBLISH,
  (const void * rmw_publisher_handle, const void * message, int64_t timestamp),
  (rmw_publisher_handle, message, timestamp))
{
  armo_ev_rmw_publish ev = {
    .rmw_publisher = ARMO_U64(rmw_publisher_handle),
    .message = ARMO_U64(message),
    .timestamp = timestamp,
    .stamp = armo_stamp(message),
  };
  armo_emit_rmw_publish(&ev);
}
#else
ROS_POINT(rmw_publish, ARMO_EV_RMW_PUBLISH_RCL, (const void * message), (message))
{
  armo_ev_rmw_publish_rcl ev = {.message = ARMO_U64(message), .stamp = armo_stamp(message)};
  armo_emit_rmw_publish_rcl(&ev);
}
#endif

ROS_POINT(
  rmw_subscription_init, ARMO_EV_RMW_SUBSCRIPTION_INIT,
  (const void * rmw_subscription_handle, const uint8_t * gid), (rmw_subscription_handle, gid))
{
  armo_ev_rmw_endpoint_init ev = {0};
  endpoint(&ev, rmw_subscription_handle, gid);
  armo_emit_rmw_subscription_init(&ev);
}

ROS_POINT(
  rcl_subscription_init, ARMO_EV_RCL_SUBSCRIPTION_INIT,
  (const void * subscription_handle, const void * node_handle, const void * rmw_subscription_handle,
   const char * topic_name, const size_t queue_depth),
  (subscription_handle, node_handle, rmw_subscription_handle, topic_name, queue_depth))
{
  armo_ev_rcl_topic_init ev = {
    .handle = ARMO_U64(subscription_handle),
    .node = ARMO_U64(node_handle),
    .rmw_handle = ARMO_U64(rmw_subscription_handle),
    .queue_depth = queue_depth,
  };
  armo_emit_rcl_subscription_init(&ev, topic_name);
}

ROS_POINT(
  rclcpp_subscription_init, ARMO_EV_RCLCPP_SUBSCRIPTION_INIT,
  (const void * subscription_handle, const void * subscription),
  (subscription_handle, subscription))
{
  armo_ev_rclcpp_subscription_init ev = {
    .subscription_handle = ARMO_U64(subscription_handle),
    .subscription = ARMO_U64(subscription),
  };
  armo_emit_rclcpp_subscription_init(&ev);
}

ROS_POINT(
  rclcpp_subscription_callback_added, ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
  (const void * subscription, const void * callback), (subscription, callback))
{
  callback_added(armo_emit_rclcpp_subscription_callback_added, subscription, callback);
}

ROS_POINT(
  rmw_take, ARMO_EV_RMW_TAKE,
  (const void * rmw_subscription_handle, const void * message, int64_t source_timestamp,
   const bool taken),
  (rmw_subscription_handle, message, source_timestamp, taken))
{
  armo_ev_rmw_take ev = {
    .rmw_subscription = ARMO_U64(rmw_subscription_handle),
    .message = ARMO_U64(message),
    .source_timestamp = source_timestamp,
    .stamp = taken ? armo_stamp(message) : 0,
    .taken = taken,
  };
  armo_emit_rmw_take(&ev);
}

ROS_POINT(rcl_take, ARMO_EV_RCL_TAKE, (const void * message), (message))
{
  armo_ev_take ev = {.message = ARMO_U64(message)};
  armo_emit_rcl_take(&ev);
}

ROS_POINT(rclcpp_take, ARMO_EV_RCLCPP_TAKE, (const void * message), (message))
{
  armo_ev_take ev = {.message = ARMO_U64(message)};
  armo_emit_rclcpp_take(&ev);
}

ROS_POINT(
  rcl_service_init, ARMO_EV_RCL_SERVICE_INIT,
  (const void * service_handle, const void * node_handle, const void * rmw_service_handle,
   const char * service_name),
  (service_handle, node_handle, rmw_service_handle, service_name))
{
  armo_ev_rcl_service_init ev = {
    .handle = ARMO_U64(service_handle),
    .node = ARMO_U64(node_handle),
    .rmw_handle = ARMO_U64(rmw_service_handle),
  };
  armo_emit_rcl_service_init(&ev, service_name);
}

ROS_POINT(
  rclcpp_service_callback_added, ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED,
  (const void * service_handle, const void * callback), (service_handle, callback))
{
  callback_added(armo_emit_rclcpp_service_callback_added, service_handle, callback);
}

ROS_POINT(
  rcl_client_init, ARMO_EV_RCL_CLIENT_INIT,
  (const void * client_handle, const void * node_handle, const void * rmw_client_handle,
   const char * service_name),
  (client_handle, node_handle, rmw_client_handle, service_name))
{
  armo_ev_rcl_service_init ev = {
    .handle = ARMO_U64(client_handle),
    .node = ARMO_U64(node_handle),
    .rmw_handle = ARMO_U64(rmw_client_handle),
  };
  armo_emit_rcl_client_init(&ev, service_name);
}

ROS_POINT(
  rcl_timer_init, ARMO_EV_RCL_TIMER_INIT, (const void * timer_handle, int64_t period),
  (timer_handle, period))
{
  armo_ev_rcl_timer_init ev = {.timer = ARMO_U64(timer_handle), .period = period};
  armo_emit_rcl_timer_init(&ev);
}

ROS_POINT(
  rclcpp_timer_callback_added, ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED,
  (const void * timer_handle, const void * callback), (timer_handle, callback))
{
  callback_added(armo_emit_rclcpp_timer_callback_added, timer_handle, callback);
}

ROS_POINT(
  rclcpp_timer_link_node, ARMO_EV_RCLCPP_TIMER_LINK_NODE,
  (const void * timer_handle, const void * node_handle), (timer_handle, node_handle))
{
  armo_ev_rclcpp_timer_link_node ev = {
    .timer = ARMO_U64(timer_handle), .node = ARMO_U64(node_handle)};
  armo_emit_rclcpp_timer_link_node(&ev);
}

ROS_POINT(
  rclcpp_callback_register, ARMO_EV_RCLCPP_CALLBACK_REGISTER,
  (const void * callback, const char * function_symbol), (callback, function_symbol))
{
  armo_ev_rclcpp_callback_register ev = {.callback = ARMO_U64(callback)};
  armo_emit_rclcpp_callback_register(&ev, function_symbol);
}

ROS_POINT(
  callback_start, ARMO_EV_CALLBACK_START, (const void * callback, const bool is_intra_process),
  (callback, is_intra_process))
{
  armo_ev_callback_start ev = {
    .callback = ARMO_U64(callback), .is_intra_process = is_intra_process};
  armo_emit_callback_start(&ev);
}

ROS_POINT(callback_end, ARMO_EV_CALLBACK_END, (const void * callback), (callback))
{
  armo_ev_handle ev = {.handle = ARMO_U64(callback)};
  armo_emit_callback_end(&ev);
}

ROS_POINT(rclcpp_executor_get_next_ready, ARMO_EV_RCLCPP_EXECUTOR_GET_NEXT_READY, (void), ())
{
  armo_emit_rclcpp_executor_get_next_ready();
}

ROS_POINT(
  rclcpp_executor_wait_for_work, ARMO_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK, (const int64_t timeout),
  (timeout))
{
  armo_ev_rclcpp_executor_wait_for_work ev = {.timeout = timeout};
  armo_emit_rclcpp_executor_wait_for_work(&ev);
}

ROS_POINT(rclcpp_executor_execute, ARMO_EV_RCLCPP_EXECUTOR_EXECUTE, (const void * handle), (handle))
{
  armo_ev_handle ev = {.handle = ARMO_U64(handle)};
  armo_emit_rclcpp_executor_execute(&ev);
}
