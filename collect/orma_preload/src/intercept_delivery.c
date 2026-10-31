// The delivery path points of the ROS 2 overhaul (docs/instrumentation.md 2.13): the broker
// anchors and the loan functions of librcl.so, the loan returns of librmw_implementation.so (rclpy
// returns its loans there directly), zenoh-c's shared memory payload, and rclpy's anchors. librcl
// and rclpy call their anchors through the PLT, so the shim catches them. A process without them
// never calls these definitions.
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define RCL "librcl.so"
#define RMW "librmw_implementation.so"
#define ZENOHC "libzenohc.so"
#define RCL_RET_OK 0
#define RCL_RET_ERROR 1
#define Z_EGENERIC (-128)

typedef int32_t rcl_ret_t;
typedef int32_t rmw_ret_t;
typedef int8_t z_result_t;

ORMA_EXPORT void rcl_ipc_trace_publish(
  const void * publisher, const void * message, int64_t source_timestamp, size_t readers);
ORMA_EXPORT void rcl_ipc_trace_take(
  const void * subscription, const void * message, const void * data, int64_t source_timestamp,
  bool local);
ORMA_EXPORT void rcl_ipc_trace_release(const void * message);
ORMA_EXPORT rcl_ret_t
rcl_borrow_loaned_message(const void * publisher, const void * type_support, void ** ros_message);
ORMA_EXPORT rcl_ret_t
rcl_publish_loaned_message(const void * publisher, void * ros_message, void * allocation);
ORMA_EXPORT rmw_ret_t
rmw_return_loaned_message_from_publisher(const void * publisher, void * loaned_message);
ORMA_EXPORT rcl_ret_t rcl_take_loaned_message(
  const void * subscription, void ** loaned_message, void * message_info, void * allocation);
ORMA_EXPORT rmw_ret_t
rmw_return_loaned_message_from_subscription(const void * subscription, void * loaned_message);
ORMA_EXPORT z_result_t z_bytes_from_shm_mut(void * bytes, void * shm);
ORMA_EXPORT void rclpy_trace_take_converted(const void * subscription, const void * message);
ORMA_EXPORT void rclpy_trace_publish_enter(const void * publisher);

void rcl_ipc_trace_publish(
  const void * publisher, const void * message, int64_t source_timestamp, size_t readers)
{
  orma_ev_rcl_ipc_publish ev = {
    .publisher = ORMA_U64(publisher),
    .message = ORMA_U64(message),
    .source_timestamp = source_timestamp,
    .readers = readers,
  };
  orma_emit_rcl_ipc_publish(&ev);
  void (*next)(const void *, const void *, int64_t, size_t) =
    ORMA_NEXT(rcl_ipc_trace_publish, RCL, ORMA_EV_RCL_IPC_PUBLISH);
  if (next != NULL) {
    next(publisher, message, source_timestamp, readers);
  }
}

void rcl_ipc_trace_take(
  const void * subscription, const void * message, const void * data, int64_t source_timestamp,
  bool local)
{
  orma_ev_rcl_ipc_take ev = {
    .subscription = ORMA_U64(subscription),
    .message = ORMA_U64(message),
    .data = ORMA_U64(data),
    .source_timestamp = source_timestamp,
    .stamp = orma_stamp(data),
    .local = local,
  };
  orma_emit_rcl_ipc_take(&ev);
  void (*next)(const void *, const void *, const void *, int64_t, bool) =
    ORMA_NEXT(rcl_ipc_trace_take, RCL, ORMA_EV_RCL_IPC_TAKE);
  if (next != NULL) {
    next(subscription, message, data, source_timestamp, local);
  }
}

void rcl_ipc_trace_release(const void * message)
{
  orma_ev_handle ev = {.handle = ORMA_U64(message)};
  orma_emit_rcl_ipc_release(&ev);
  void (*next)(const void *) = ORMA_NEXT(rcl_ipc_trace_release, RCL, ORMA_EV_RCL_IPC_RELEASE);
  if (next != NULL) {
    next(message);
  }
}

static void loan(
  void (*emit)(const orma_ev_rcl_loan *), const void * endpoint, const void * message,
  rcl_ret_t ret, bool subscription)
{
  orma_ev_rcl_loan ev = {
    .endpoint = ORMA_U64(endpoint),
    .message = ORMA_U64(message),
    .ret = ret,
    .subscription = subscription,
  };
  emit(&ev);
}

rcl_ret_t rcl_borrow_loaned_message(
  const void * publisher, const void * type_support, void ** ros_message)
{
  rcl_ret_t (*next)(const void *, const void *, void **) =
    ORMA_NEXT(rcl_borrow_loaned_message, RCL, ORMA_EV_RCL_LOAN_BORROW);
  rcl_ret_t ret = next != NULL ? next(publisher, type_support, ros_message) : RCL_RET_ERROR;
  const void * message = ret == RCL_RET_OK && ros_message != NULL ? *ros_message : NULL;
  loan(orma_emit_rcl_loan_borrow, publisher, message, ret, false);
  return ret;
}

rcl_ret_t rcl_publish_loaned_message(const void * publisher, void * ros_message, void * allocation)
{
  loan(orma_emit_rcl_loan_publish, publisher, ros_message, RCL_RET_OK, false);
  rcl_ret_t (*next)(const void *, void *, void *) =
    ORMA_NEXT(rcl_publish_loaned_message, RCL, ORMA_EV_RCL_LOAN_PUBLISH);
  return next != NULL ? next(publisher, ros_message, allocation) : RCL_RET_ERROR;
}

rmw_ret_t rmw_return_loaned_message_from_publisher(const void * publisher, void * loaned_message)
{
  loan(orma_emit_rmw_loan_return, publisher, loaned_message, RCL_RET_OK, false);
  rmw_ret_t (*next)(const void *, void *) =
    ORMA_NEXT(rmw_return_loaned_message_from_publisher, RMW, ORMA_EV_RMW_LOAN_RETURN);
  return next != NULL ? next(publisher, loaned_message) : RCL_RET_ERROR;
}

rcl_ret_t rcl_take_loaned_message(
  const void * subscription, void ** loaned_message, void * message_info, void * allocation)
{
  loan(orma_emit_rcl_loan_take, subscription, NULL, RCL_RET_OK, true);
  rcl_ret_t (*next)(const void *, void **, void *, void *) =
    ORMA_NEXT(rcl_take_loaned_message, RCL, ORMA_EV_RCL_LOAN_TAKE);
  return next != NULL ? next(subscription, loaned_message, message_info, allocation)
                      : RCL_RET_ERROR;
}

rmw_ret_t rmw_return_loaned_message_from_subscription(
  const void * subscription, void * loaned_message)
{
  loan(orma_emit_rmw_loan_return, subscription, loaned_message, RCL_RET_OK, true);
  rmw_ret_t (*next)(const void *, void *) =
    ORMA_NEXT(rmw_return_loaned_message_from_subscription, RMW, ORMA_EV_RMW_LOAN_RETURN);
  return next != NULL ? next(subscription, loaned_message) : RCL_RET_ERROR;
}

z_result_t z_bytes_from_shm_mut(void * bytes, void * shm)
{
  orma_ev_handle ev = {.handle = ORMA_U64(bytes)};
  orma_emit_zenoh_shm_payload(&ev);
  z_result_t (*next)(void *, void *) =
    ORMA_NEXT(z_bytes_from_shm_mut, ZENOHC, ORMA_EV_ZENOH_SHM_PAYLOAD);
  return next != NULL ? next(bytes, shm) : Z_EGENERIC;
}

void rclpy_trace_take_converted(const void * subscription, const void * message)
{
  orma_ev_rclpy_take ev = {.subscription = ORMA_U64(subscription), .message = ORMA_U64(message)};
  orma_emit_rclpy_take_converted(&ev);
  void (*next)(const void *, const void *) =
    ORMA_NEXT(rclpy_trace_take_converted, NULL, ORMA_EV_RCLPY_TAKE_CONVERTED);
  if (next != NULL) {
    next(subscription, message);
  }
}

void rclpy_trace_publish_enter(const void * publisher)
{
  orma_ev_handle ev = {.handle = ORMA_U64(publisher)};
  orma_emit_rclpy_publish_enter(&ev);
  void (*next)(const void *) =
    ORMA_NEXT(rclpy_trace_publish_enter, NULL, ORMA_EV_RCLPY_PUBLISH_ENTER);
  if (next != NULL) {
    next(publisher);
  }
}
