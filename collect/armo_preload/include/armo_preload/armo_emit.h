// The seam between armo_intercept, which catches the functions of design 2, and a backend
// (libarmo_preload.so writes shared memory, libarmo_lttng.so LTTng events). The intercept calls
// armo_emit_<event> once per caught call with the record payload filled; it has already read the
// stamp and callback code words, answered gated enabled_ functions and checked the flux
// contract. A backend reads the clock, keeps per-thread state and defines every function here.
// A string argument may be NULL, which the backend records as an empty string.
#ifndef ARMO_PRELOAD_ARMO_EMIT_H
#define ARMO_PRELOAD_ARMO_EMIT_H

#include "armo_preload/armo_events.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

void armo_emit_rcl_init(const armo_ev_rcl_init * ev);
void armo_emit_rcl_node_init(
  const armo_ev_rcl_node_init * ev, const char * node_name, const char * node_namespace);
void armo_emit_rmw_publisher_init(const armo_ev_rmw_endpoint_init * ev);
void armo_emit_rcl_publisher_init(const armo_ev_rcl_topic_init * ev, const char * topic_name);
void armo_emit_rclcpp_publish(const armo_ev_publish * ev);
void armo_emit_rclcpp_intra_publish(const armo_ev_publish * ev);
void armo_emit_rcl_publish(const armo_ev_publish * ev);
void armo_emit_rmw_publish(const armo_ev_rmw_publish * ev);
void armo_emit_rmw_publish_rcl(const armo_ev_rmw_publish_rcl * ev);
void armo_emit_rmw_subscription_init(const armo_ev_rmw_endpoint_init * ev);
void armo_emit_rcl_subscription_init(const armo_ev_rcl_topic_init * ev, const char * topic_name);
void armo_emit_rclcpp_subscription_init(const armo_ev_rclcpp_subscription_init * ev);
void armo_emit_rclcpp_subscription_callback_added(const armo_ev_callback_added * ev);
void armo_emit_rmw_take(const armo_ev_rmw_take * ev);
void armo_emit_rcl_take(const armo_ev_take * ev);
void armo_emit_rclcpp_take(const armo_ev_take * ev);
void armo_emit_rcl_service_init(const armo_ev_rcl_service_init * ev, const char * service_name);
void armo_emit_rclcpp_service_callback_added(const armo_ev_callback_added * ev);
void armo_emit_rcl_client_init(const armo_ev_rcl_service_init * ev, const char * service_name);
void armo_emit_rcl_timer_init(const armo_ev_rcl_timer_init * ev);
void armo_emit_rclcpp_timer_callback_added(const armo_ev_callback_added * ev);
void armo_emit_rclcpp_timer_link_node(const armo_ev_rclcpp_timer_link_node * ev);
void armo_emit_rclcpp_callback_register(
  const armo_ev_rclcpp_callback_register * ev, const char * function_symbol);
void armo_emit_callback_start(const armo_ev_callback_start * ev);
void armo_emit_callback_end(const armo_ev_handle * ev);
void armo_emit_rclcpp_executor_get_next_ready(void);
void armo_emit_rclcpp_executor_wait_for_work(const armo_ev_rclcpp_executor_wait_for_work * ev);
void armo_emit_rclcpp_executor_execute(const armo_ev_handle * ev);
void armo_emit_rmw_client_init(const armo_ev_rmw_endpoint_init * ev);
void armo_emit_rmw_send_request(const armo_ev_rmw_send_request * ev);
void armo_emit_rmw_take_request(const armo_ev_rmw_take_request * ev);
void armo_emit_rmw_send_response(const armo_ev_rmw_send_response * ev);
void armo_emit_rmw_take_response(const armo_ev_rmw_take_response * ev);
void armo_emit_rcl_wait_enter(const armo_ev_rcl_wait_enter * ev);
void armo_emit_rcl_wait_exit(const armo_ev_rcl_wait_exit * ev);
void armo_emit_rcl_timer_fini(const armo_ev_handle * ev);
void armo_emit_rclcpp_publisher_base(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_publisher_setup_intra(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_subscription_setup_intra(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_ipm_insert_sub_id_for_pub(
  const armo_ev_rclcpp_ipm_insert_sub_id_for_pub * ev);
void armo_emit_rclcpp_group_add_subscription(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_group_add_timer(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_executor(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_collector(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_collector_add_group_to_collection(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_collector_add_group(const armo_ev_rclcpp_object * ev);
void armo_emit_rclcpp_executor_add_group(const armo_ev_rclcpp_object * ev);
void armo_emit_callback_code(const armo_ev_callback_code * ev);
void armo_emit_flux_channel_init(const armo_ev_flux_channel_init * ev, const char * segment);
void armo_emit_flux_claim(const armo_ev_flux_claim * ev);
void armo_emit_flux_loan(const armo_ev_flux_loan * ev);
void armo_emit_flux_loan_end(const armo_ev_flux_loan_end * ev);
void armo_emit_flux_commit(const armo_ev_flux_commit * ev);
void armo_emit_flux_take(const armo_ev_flux_take * ev);
void armo_emit_flux_release(const armo_ev_flux_release * ev);
void armo_emit_flux_refused(const armo_ev_flux_refused * ev);
void armo_emit_flux_wait(const armo_ev_flux_wait * ev);
void armo_emit_flux_wake(const armo_ev_flux_wake * ev);
void armo_emit_flux_callback_start(const armo_ev_flux_callback_start * ev);
void armo_emit_flux_callback_end(const armo_ev_flux_callback_end * ev, const char * segment);
void armo_emit_flux_endpoint_init(
  const armo_ev_flux_endpoint_init * ev, const char * node, const char * key,
  const char * signpost);
void armo_emit_flux_py_gil(const armo_ev_flux_py_gil * ev);
void armo_emit_flux_py_ready(const armo_ev_flux_py_ready * ev);
void armo_emit_flux_record_drop(const armo_ev_flux_record_drop * ev);
void armo_emit_flux_gpu_wedged(const armo_ev_flux_gpu_wedged * ev);
void armo_emit_dds_write(const armo_ev_dds_write * ev);
void armo_emit_cuda_call(const armo_ev_cuda_call * ev);
void armo_emit_span_begin(const armo_ev_span * ev, const char * name);
void armo_emit_span_end(const armo_ev_span * ev);

// The intercept point of `event` found no original function (armo_shm_header.missing).
void armo_backend_missing(uint16_t event);

// malloc attribution (design 15.3): whether it is on, and one allocation or release of `bytes`.
bool armo_backend_malloc_on(void);
void armo_backend_alloc(size_t bytes);
void armo_backend_free(size_t bytes);

// Whether CUDA driver calls (design 20.1) and DDS write calls (20.2) are timed.
bool armo_backend_gpu_on(void);
bool armo_backend_dds_on(void);

#endif  // ARMO_PRELOAD_ARMO_EMIT_H
