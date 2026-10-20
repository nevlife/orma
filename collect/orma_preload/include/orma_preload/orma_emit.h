// The seam between orma_intercept, which catches the functions of design 2, and a backend
// (liborma_preload.so writes shared memory, liborma_lttng.so LTTng events). The intercept calls
// orma_emit_<event> once per caught call with the record payload filled; it has already read the
// stamp and callback code words, answered gated enabled_ functions and checked the flux
// contract. A backend reads the clock, keeps per-thread state and defines every function here.
// A string argument may be NULL, which the backend records as an empty string.
#ifndef ORMA_PRELOAD_ORMA_EMIT_H
#define ORMA_PRELOAD_ORMA_EMIT_H

#include "orma_preload/orma_events.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

void orma_emit_rcl_init(const orma_ev_rcl_init * ev);
void orma_emit_rcl_node_init(
  const orma_ev_rcl_node_init * ev, const char * node_name, const char * node_namespace);
void orma_emit_rmw_publisher_init(const orma_ev_rmw_endpoint_init * ev);
void orma_emit_rcl_publisher_init(const orma_ev_rcl_topic_init * ev, const char * topic_name);
void orma_emit_rclcpp_publish(const orma_ev_publish * ev);
void orma_emit_rclcpp_intra_publish(const orma_ev_publish * ev);
void orma_emit_rcl_publish(const orma_ev_publish * ev);
void orma_emit_rmw_publish(const orma_ev_rmw_publish * ev);
void orma_emit_rmw_publish_rcl(const orma_ev_rmw_publish_rcl * ev);
void orma_emit_rmw_subscription_init(const orma_ev_rmw_endpoint_init * ev);
void orma_emit_rcl_subscription_init(const orma_ev_rcl_topic_init * ev, const char * topic_name);
void orma_emit_rclcpp_subscription_init(const orma_ev_rclcpp_subscription_init * ev);
void orma_emit_rclcpp_subscription_callback_added(const orma_ev_callback_added * ev);
void orma_emit_rmw_take(const orma_ev_rmw_take * ev);
void orma_emit_rcl_take(const orma_ev_take * ev);
void orma_emit_rclcpp_take(const orma_ev_take * ev);
void orma_emit_rcl_service_init(const orma_ev_rcl_service_init * ev, const char * service_name);
void orma_emit_rclcpp_service_callback_added(const orma_ev_callback_added * ev);
void orma_emit_rcl_client_init(const orma_ev_rcl_service_init * ev, const char * service_name);
void orma_emit_rcl_timer_init(const orma_ev_rcl_timer_init * ev);
void orma_emit_rclcpp_timer_callback_added(const orma_ev_callback_added * ev);
void orma_emit_rclcpp_timer_link_node(const orma_ev_rclcpp_timer_link_node * ev);
void orma_emit_rclcpp_callback_register(
  const orma_ev_rclcpp_callback_register * ev, const char * function_symbol);
void orma_emit_callback_start(const orma_ev_callback_start * ev);
void orma_emit_callback_end(const orma_ev_handle * ev);
void orma_emit_rclcpp_executor_get_next_ready(void);
void orma_emit_rclcpp_executor_wait_for_work(const orma_ev_rclcpp_executor_wait_for_work * ev);
void orma_emit_rclcpp_executor_execute(const orma_ev_handle * ev);
void orma_emit_rcl_wait_enter(const orma_ev_rcl_wait_enter * ev);
void orma_emit_rcl_wait_exit(const orma_ev_rcl_wait_exit * ev);
void orma_emit_rcl_timer_fini(const orma_ev_handle * ev);
void orma_emit_rclcpp_publisher_base(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_publisher_setup_intra(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_subscription_setup_intra(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_ipm_insert_sub_id_for_pub(
  const orma_ev_rclcpp_ipm_insert_sub_id_for_pub * ev);
void orma_emit_rclcpp_group_add_subscription(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_group_add_timer(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_executor(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_collector(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_collector_add_group_to_collection(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_collector_add_group(const orma_ev_rclcpp_object * ev);
void orma_emit_rclcpp_executor_add_group(const orma_ev_rclcpp_object * ev);
void orma_emit_callback_code(const orma_ev_callback_code * ev);
void orma_emit_flux_channel_init(const orma_ev_flux_channel_init * ev, const char * segment);
void orma_emit_flux_claim(const orma_ev_flux_claim * ev);
void orma_emit_flux_loan(const orma_ev_flux_loan * ev);
void orma_emit_flux_loan_end(const orma_ev_flux_loan_end * ev);
void orma_emit_flux_commit(const orma_ev_flux_commit * ev);
void orma_emit_flux_take(const orma_ev_flux_take * ev);
void orma_emit_flux_release(const orma_ev_flux_release * ev);
void orma_emit_flux_refused(const orma_ev_flux_refused * ev);
void orma_emit_flux_wait(const orma_ev_flux_wait * ev);
void orma_emit_flux_wake(const orma_ev_flux_wake * ev);
void orma_emit_flux_callback_start(const orma_ev_flux_callback_start * ev);
void orma_emit_flux_callback_end(const orma_ev_flux_callback_end * ev, const char * segment);
void orma_emit_flux_endpoint_init(
  const orma_ev_flux_endpoint_init * ev, const char * node, const char * key,
  const char * signpost);
void orma_emit_flux_py_gil(const orma_ev_flux_py_gil * ev);
void orma_emit_flux_py_ready(const orma_ev_flux_py_ready * ev);
void orma_emit_flux_record_drop(const orma_ev_flux_record_drop * ev);
void orma_emit_flux_gpu_wedged(const orma_ev_flux_gpu_wedged * ev);
void orma_emit_dds_write(const orma_ev_dds_write * ev);
void orma_emit_cuda_call(const orma_ev_cuda_call * ev);
void orma_emit_span_begin(const orma_ev_span * ev, const char * name);
void orma_emit_span_end(const orma_ev_span * ev);

// The intercept point of `event` found no original function (orma_shm_header.missing).
void orma_backend_missing(uint16_t event);

// malloc attribution (design 15.3): whether it is on, and one allocation or release of `bytes`.
bool orma_backend_malloc_on(void);
void orma_backend_alloc(size_t bytes);
void orma_backend_free(size_t bytes);

// Whether CUDA driver calls (design 20.1) and DDS write calls (20.2) are timed.
bool orma_backend_gpu_on(void);
bool orma_backend_dds_on(void);

#endif  // ORMA_PRELOAD_ORMA_EMIT_H
