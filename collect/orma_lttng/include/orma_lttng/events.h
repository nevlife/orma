// The events of provider orma: every orma_emit_<name> of orma_emit.h but the flux ones, which
// provider flux carries with their arguments as fields, and rclcpp_executor_get_next_ready, which
// has no payload. X<strings>(name, payload type, strings...) in catalog order; R marks the events
// that also have a provider ros2 event where libtracetools has no LTTng (ros2_tp.h).
#ifndef ORMA_LTTNG_EVENTS_H
#define ORMA_LTTNG_EVENTS_H

#define ORMA_LTTNG_EVENTS(X0, X1, X2, R0, R1, R2)                                \
  R0(rcl_init, orma_ev_rcl_init)                                                 \
  R2(rcl_node_init, orma_ev_rcl_node_init, node_name, node_namespace)            \
  R0(rmw_publisher_init, orma_ev_rmw_endpoint_init)                              \
  R1(rcl_publisher_init, orma_ev_rcl_topic_init, topic_name)                     \
  R0(rclcpp_publish, orma_ev_publish)                                            \
  X0(rclcpp_intra_publish, orma_ev_publish)                                      \
  R0(rcl_publish, orma_ev_publish)                                               \
  X0(rmw_publish, orma_ev_rmw_publish)                                           \
  R0(rmw_publish_rcl, orma_ev_rmw_publish_rcl)                                   \
  R0(rmw_subscription_init, orma_ev_rmw_endpoint_init)                           \
  R1(rcl_subscription_init, orma_ev_rcl_topic_init, topic_name)                  \
  R0(rclcpp_subscription_init, orma_ev_rclcpp_subscription_init)                 \
  R0(rclcpp_subscription_callback_added, orma_ev_callback_added)                 \
  R0(rmw_take, orma_ev_rmw_take)                                                 \
  R0(rcl_take, orma_ev_take)                                                     \
  R0(rclcpp_take, orma_ev_take)                                                  \
  R1(rcl_service_init, orma_ev_rcl_service_init, service_name)                   \
  R0(rclcpp_service_callback_added, orma_ev_callback_added)                      \
  R1(rcl_client_init, orma_ev_rcl_service_init, service_name)                    \
  R0(rcl_timer_init, orma_ev_rcl_timer_init)                                     \
  R0(rclcpp_timer_callback_added, orma_ev_callback_added)                        \
  R0(rclcpp_timer_link_node, orma_ev_rclcpp_timer_link_node)                     \
  R1(rclcpp_callback_register, orma_ev_rclcpp_callback_register, symbol)         \
  R0(callback_start, orma_ev_callback_start)                                     \
  R0(callback_end, orma_ev_handle)                                               \
  R0(rclcpp_executor_wait_for_work, orma_ev_rclcpp_executor_wait_for_work)       \
  R0(rclcpp_executor_execute, orma_ev_handle)                                    \
  X0(rcl_wait_enter, orma_ev_rcl_wait_enter)                                     \
  X0(rcl_wait_exit, orma_ev_rcl_wait_exit)                                       \
  X0(rcl_timer_fini, orma_ev_handle)                                             \
  X0(rclcpp_publisher_base, orma_ev_rclcpp_object)                               \
  X0(rclcpp_publisher_setup_intra, orma_ev_rclcpp_object)                        \
  X0(rclcpp_subscription_setup_intra, orma_ev_rclcpp_object)                     \
  X0(rclcpp_ipm_insert_sub_id_for_pub, orma_ev_rclcpp_ipm_insert_sub_id_for_pub) \
  X0(rclcpp_group_add_subscription, orma_ev_rclcpp_object)                       \
  X0(rclcpp_group_add_timer, orma_ev_rclcpp_object)                              \
  X0(rclcpp_executor, orma_ev_rclcpp_object)                                     \
  X0(rclcpp_collector, orma_ev_rclcpp_object)                                    \
  X0(rclcpp_collector_add_group_to_collection, orma_ev_rclcpp_object)            \
  X0(rclcpp_collector_add_group, orma_ev_rclcpp_object)                          \
  X0(callback_code, orma_ev_callback_code)                                       \
  X0(rclcpp_executor_add_group, orma_ev_rclcpp_object)                           \
  X0(dds_write, orma_ev_dds_write)                                               \
  X0(rcl_ipc_publish, orma_ev_rcl_ipc_publish)                                   \
  X0(rcl_ipc_take, orma_ev_rcl_ipc_take)                                         \
  X0(rcl_ipc_release, orma_ev_handle)                                            \
  X0(rcl_loan_borrow, orma_ev_rcl_loan)                                          \
  X0(rcl_loan_publish, orma_ev_rcl_loan)                                         \
  X0(rcl_loan_take, orma_ev_rcl_loan)                                            \
  X0(rcl_loan_return, orma_ev_rcl_loan)                                          \
  X0(zenoh_shm_payload, orma_ev_handle)                                          \
  X0(rclpy_take_converted, orma_ev_rclpy_take)                                   \
  X0(rclpy_publish_enter, orma_ev_handle)                                        \
  X0(cuda_call, orma_ev_cuda_call)                                               \
  X1(span_begin, orma_ev_span, name)                                             \
  X0(span_end, orma_ev_span)

#endif  // ORMA_LTTNG_EVENTS_H
