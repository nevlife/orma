// Stands in for the overhaul's rcl broker anchors, rclpy's anchors and zenoh-c's
// z_bytes_from_shm_mut, each recording that it was reached. The rcl loan functions are librcl's
// own.
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define STUB __attribute__((visibility("default"), noinline))

static const char * last;

STUB const char * stub_last(void)
{
  return last;
}

STUB void rcl_ipc_trace_publish(
  const void * publisher, const void * message, int64_t source_timestamp, size_t readers)
{
  (void)publisher, (void)message, (void)source_timestamp, (void)readers;
  last = "rcl_ipc_trace_publish";
}

STUB void rcl_ipc_trace_take(
  const void * subscription, const void * message, const void * data, int64_t source_timestamp,
  bool local)
{
  (void)subscription, (void)message, (void)data, (void)source_timestamp, (void)local;
  last = "rcl_ipc_trace_take";
}

STUB void rcl_ipc_trace_release(const void * message)
{
  (void)message;
  last = "rcl_ipc_trace_release";
}

STUB int8_t z_bytes_from_shm_mut(void * bytes, void * shm)
{
  (void)bytes, (void)shm;
  last = "z_bytes_from_shm_mut";
  return 0;
}

STUB void rclpy_trace_take_converted(const void * subscription, const void * message)
{
  (void)subscription, (void)message;
  last = "rclpy_trace_take_converted";
}

STUB void rclpy_trace_publish_enter(const void * publisher)
{
  (void)publisher;
  last = "rclpy_trace_publish_enter";
}
