// The events of provider armo: every armo_emit_<name> of armo_emit.h but the flux ones, which
// provider flux carries with their arguments as fields, and rclcpp_executor_get_next_ready, which
// has no payload. X<strings>(name, payload type, strings...) in catalog order; R marks the events
// that also have a provider ros2 event where libtracetools has no LTTng (ros2_tp.h).
#ifndef ARMO_LTTNG_EVENTS_H
#define ARMO_LTTNG_EVENTS_H

#define ARMO_LTTNG_EVENTS(X0, X1, X2, R0, R1, R2)                                \
  R0(rcl_init, armo_ev_rcl_init)                                                 \
  R2(rcl_node_init, armo_ev_rcl_node_init, node_name, node_namespace)            \
  R0(rmw_publisher_init, armo_ev_rmw_endpoint_init)                              \
  R1(rcl_publisher_init, armo_ev_rcl_topic_init, topic_name)                     \
  R0(rclcpp_publish, armo_ev_publish)                                            \
  X0(rclcpp_intra_publish, armo_ev_publish)                                      \
  R0(rcl_publish, armo_ev_publish)                                               \
  X0(rmw_publish, armo_ev_rmw_publish)                                           \
  R0(rmw_publish_rcl, armo_ev_rmw_publish_rcl)                                   \
  R0(rmw_subscription_init, armo_ev_rmw_endpoint_init)                           \
  R1(rcl_subscription_init, armo_ev_rcl_topic_init, topic_name)                  \
  R0(rclcpp_subscription_init, armo_ev_rclcpp_subscription_init)                 \
  R0(rclcpp_subscription_callback_added, armo_ev_callback_added)                 \
  R0(rmw_take, armo_ev_rmw_take)                                                 \
  R0(rcl_take, armo_ev_take)                                                     \
  R0(rclcpp_take, armo_ev_take)                                                  \
  R1(rcl_service_init, armo_ev_rcl_service_init, service_name)                   \
  R0(rclcpp_service_callback_added, armo_ev_callback_added)                      \
  R1(rcl_client_init, armo_ev_rcl_service_init, service_name)                    \
  R0(rcl_timer_init, armo_ev_rcl_timer_init)                                     \
  R0(rclcpp_timer_callback_added, armo_ev_callback_added)                        \
  R0(rclcpp_timer_link_node, armo_ev_rclcpp_timer_link_node)                     \
  R1(rclcpp_callback_register, armo_ev_rclcpp_callback_register, symbol)         \
  R0(callback_start, armo_ev_callback_start)                                     \
  R0(callback_end, armo_ev_handle)                                               \
  R0(rclcpp_executor_wait_for_work, armo_ev_rclcpp_executor_wait_for_work)       \
  R0(rclcpp_executor_execute, armo_ev_handle)                                    \
  X0(rcl_wait_enter, armo_ev_rcl_wait_enter)                                     \
  X0(rcl_wait_exit, armo_ev_rcl_wait_exit)                                       \
  X0(rcl_timer_fini, armo_ev_handle)                                             \
  X0(rclcpp_publisher_base, armo_ev_rclcpp_object)                               \
  X0(rclcpp_publisher_setup_intra, armo_ev_rclcpp_object)                        \
  X0(rclcpp_subscription_setup_intra, armo_ev_rclcpp_object)                     \
  X0(rclcpp_ipm_insert_sub_id_for_pub, armo_ev_rclcpp_ipm_insert_sub_id_for_pub) \
  X0(rclcpp_group_add_subscription, armo_ev_rclcpp_object)                       \
  X0(rclcpp_group_add_timer, armo_ev_rclcpp_object)                              \
  X0(rclcpp_executor, armo_ev_rclcpp_object)                                     \
  X0(rclcpp_collector, armo_ev_rclcpp_object)                                    \
  X0(rclcpp_collector_add_group_to_collection, armo_ev_rclcpp_object)            \
  X0(rclcpp_collector_add_group, armo_ev_rclcpp_object)                          \
  X0(callback_code, armo_ev_callback_code)                                       \
  X0(rclcpp_executor_add_group, armo_ev_rclcpp_object)                           \
  X0(dds_write, armo_ev_dds_write)                                               \
  X0(cuda_call, armo_ev_cuda_call)                                               \
  X1(span_begin, armo_ev_span, name)                                             \
  X0(span_end, armo_ev_span)

#endif  // ARMO_LTTNG_EVENTS_H
