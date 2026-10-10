#undef LTTNG_UST_TRACEPOINT_PROVIDER
#define LTTNG_UST_TRACEPOINT_PROVIDER ros2

#undef LTTNG_UST_TRACEPOINT_INCLUDE
#define LTTNG_UST_TRACEPOINT_INCLUDE "armo_lttng/ros2_tp.h"

#if !defined(ARMO_LTTNG_ROS2_TP_H) || defined(LTTNG_UST_TRACEPOINT_HEADER_MULTI_READ)
#define ARMO_LTTNG_ROS2_TP_H

#include <lttng/tracepoint.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Provider ros2 for a libtracetools built without LTTng (Humble, design 2.7.1): the events armo
// intercepts, with the names, types and fields of Humble's tracetools 4.1 tp_call.h, so that
// ros2_tracing's readers take them as they take libtracetools' own.

#define ARMO_ROS2_GID_SIZE 24u

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_init, LTTNG_UST_TP_ARGS(const void *, context_handle_arg, const char *, version_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, context_handle, context_handle_arg)
                        lttng_ust_field_string(version, version_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_node_init,
  LTTNG_UST_TP_ARGS(
    const void *, node_handle_arg, const void *, rmw_handle_arg, const char *, node_name_arg,
    const char *, namespace_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, node_handle, node_handle_arg)
                        lttng_ust_field_integer_hex(const void *, rmw_handle, rmw_handle_arg)
                          lttng_ust_field_string(node_name, node_name_arg)
                            lttng_ust_field_string(namespace, namespace_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rmw_publisher_init,
  LTTNG_UST_TP_ARGS(const void *, rmw_publisher_handle_arg, const uint8_t *, gid_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, rmw_publisher_handle, rmw_publisher_handle_arg)
      lttng_ust_field_array(uint8_t, gid, gid_arg, ARMO_ROS2_GID_SIZE)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_publisher_init,
  LTTNG_UST_TP_ARGS(
    const void *, publisher_handle_arg, const void *, node_handle_arg, const void *,
    rmw_publisher_handle_arg, const char *, topic_name_arg, const size_t, queue_depth_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, publisher_handle, publisher_handle_arg)
      lttng_ust_field_integer_hex(const void *, node_handle, node_handle_arg)
        lttng_ust_field_integer_hex(const void *, rmw_publisher_handle, rmw_publisher_handle_arg)
          lttng_ust_field_string(topic_name, topic_name_arg)
            lttng_ust_field_integer(const size_t, queue_depth, queue_depth_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_publish, LTTNG_UST_TP_ARGS(const void *, message_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, message, message_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_publish,
  LTTNG_UST_TP_ARGS(const void *, publisher_handle_arg, const void *, message_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, publisher_handle, publisher_handle_arg)
      lttng_ust_field_integer_hex(const void *, message, message_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rmw_publish, LTTNG_UST_TP_ARGS(const void *, message_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, message, message_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rmw_subscription_init,
  LTTNG_UST_TP_ARGS(const void *, rmw_subscription_handle_arg, const uint8_t *, gid_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, rmw_subscription_handle, rmw_subscription_handle_arg)
      lttng_ust_field_array(uint8_t, gid, gid_arg, ARMO_ROS2_GID_SIZE)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_subscription_init,
  LTTNG_UST_TP_ARGS(
    const void *, subscription_handle_arg, const void *, node_handle_arg, const void *,
    rmw_subscription_handle_arg, const char *, topic_name_arg, const size_t, queue_depth_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, subscription_handle, subscription_handle_arg)
      lttng_ust_field_integer_hex(const void *, node_handle, node_handle_arg)
        lttng_ust_field_integer_hex(
          const void *, rmw_subscription_handle, rmw_subscription_handle_arg)
          lttng_ust_field_string(topic_name, topic_name_arg)
            lttng_ust_field_integer(const size_t, queue_depth, queue_depth_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_subscription_init,
  LTTNG_UST_TP_ARGS(const void *, subscription_handle_arg, const void *, subscription_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, subscription_handle, subscription_handle_arg)
      lttng_ust_field_integer_hex(const void *, subscription, subscription_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_subscription_callback_added,
  LTTNG_UST_TP_ARGS(const void *, subscription_arg, const void *, callback_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, subscription, subscription_arg)
                        lttng_ust_field_integer_hex(const void *, callback, callback_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rmw_take,
  LTTNG_UST_TP_ARGS(
    const void *, rmw_subscription_handle_arg, const void *, message_arg, int64_t,
    source_timestamp_arg, const bool, taken_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, rmw_subscription_handle, rmw_subscription_handle_arg)
      lttng_ust_field_integer_hex(const void *, message, message_arg)
        lttng_ust_field_integer(int64_t, source_timestamp, source_timestamp_arg)
          lttng_ust_field_integer(int, taken, (taken_arg ? 1 : 0))))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_take, LTTNG_UST_TP_ARGS(const void *, message_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, message, message_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_take, LTTNG_UST_TP_ARGS(const void *, message_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, message, message_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_service_init,
  LTTNG_UST_TP_ARGS(
    const void *, service_handle_arg, const void *, node_handle_arg, const void *,
    rmw_service_handle_arg, const char *, service_name_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, service_handle, service_handle_arg)
      lttng_ust_field_integer_hex(const void *, node_handle, node_handle_arg)
        lttng_ust_field_integer_hex(const void *, rmw_service_handle, rmw_service_handle_arg)
          lttng_ust_field_string(service_name, service_name_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_service_callback_added,
  LTTNG_UST_TP_ARGS(const void *, service_handle_arg, const void *, callback_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, service_handle, service_handle_arg)
                        lttng_ust_field_integer_hex(const void *, callback, callback_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_client_init,
  LTTNG_UST_TP_ARGS(
    const void *, client_handle_arg, const void *, node_handle_arg, const void *,
    rmw_client_handle_arg, const char *, service_name_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, client_handle, client_handle_arg)
      lttng_ust_field_integer_hex(const void *, node_handle, node_handle_arg)
        lttng_ust_field_integer_hex(const void *, rmw_client_handle, rmw_client_handle_arg)
          lttng_ust_field_string(service_name, service_name_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rcl_timer_init, LTTNG_UST_TP_ARGS(const void *, timer_handle_arg, int64_t, period_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, timer_handle, timer_handle_arg)
                        lttng_ust_field_integer(int64_t, period, period_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_timer_callback_added,
  LTTNG_UST_TP_ARGS(const void *, timer_handle_arg, const void *, callback_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, timer_handle, timer_handle_arg)
                        lttng_ust_field_integer_hex(const void *, callback, callback_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_timer_link_node,
  LTTNG_UST_TP_ARGS(const void *, timer_handle_arg, const void *, node_handle_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, timer_handle, timer_handle_arg)
                        lttng_ust_field_integer_hex(const void *, node_handle, node_handle_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_callback_register,
  LTTNG_UST_TP_ARGS(const void *, callback_arg, const char *, symbol_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, callback, callback_arg)
                        lttng_ust_field_string(symbol, symbol_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, callback_start,
  LTTNG_UST_TP_ARGS(const void *, callback_arg, const bool, is_intra_process_arg),
  LTTNG_UST_TP_FIELDS(
    lttng_ust_field_integer_hex(const void *, callback, callback_arg)
      lttng_ust_field_integer(int, is_intra_process, (is_intra_process_arg ? 1 : 0))))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, callback_end, LTTNG_UST_TP_ARGS(const void *, callback_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, callback, callback_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_executor_get_next_ready, LTTNG_UST_TP_ARGS(), LTTNG_UST_TP_FIELDS())

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_executor_wait_for_work, LTTNG_UST_TP_ARGS(const int64_t, timeout_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer(int64_t, timeout, timeout_arg)))

LTTNG_UST_TRACEPOINT_EVENT(
  ros2, rclcpp_executor_execute, LTTNG_UST_TP_ARGS(const void *, handle_arg),
  LTTNG_UST_TP_FIELDS(lttng_ust_field_integer_hex(const void *, handle, handle_arg)))

#endif  // ARMO_LTTNG_ROS2_TP_H

#include <lttng/tracepoint-event.h>
