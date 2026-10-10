// Calls every ros_trace_* this distro's libtracetools exports and, when built with flux2, every
// flux2_trace_* function ROUNDS times with known arguments, and prints for each armo event the
// count it should cause: "<provider:event> <count>". lttng_events.py runs it under
// LD_PRELOAD=libarmo_lttng.so in an LTTng session and compares.
//
//     lttng_caller burst N     N callback_start, callback_end pairs, busy 6 ms of every 10 ms
//     (lifecycle.py) lttng_caller tick        a pair every 50 ms until killed
#include "armo_preload/armo_tracepoints.h"
#include "tracetools/tracetools.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#if ARMO_FLUX2
#include "flux2/trace.h"
#endif

#define ROUNDS 3
#define H(i) ((const void *)(uintptr_t)(0x1000 + (i)))
#define MS 1000000L

static long now_ns(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return ts.tv_sec * 1000 * MS + ts.tv_nsec;
}

// At most 60 % of the CPU it runs on: a 6-core 100 % burst cut this machine's power (2026-10-09).
static void burst(long n)
{
  long period = now_ns();
  for (long k = 0; k < n; k++) {
    ros_trace_callback_start(H(0), false);
    ros_trace_callback_end(H(0));
    if (k % 1024 == 0 && now_ns() - period >= 6 * MS) {
      period += 10 * MS;
      struct timespec until = {.tv_sec = period / (1000 * MS), .tv_nsec = period % (1000 * MS)};
      clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &until, NULL);
      period = now_ns();
    }
  }
}

static void expect(const char * event, int per_round)
{
  printf("%s %d\n", event, per_round * ROUNDS);
}

int main(int argc, char ** argv)
{
  if (argc > 2 && strcmp(argv[1], "burst") == 0) {
    burst(strtol(argv[2], NULL, 10));
    return 0;
  }
  if (argc > 1 && strcmp(argv[1], "tick") == 0) {
    for (;;) {
      ros_trace_callback_start(H(0), false);
      ros_trace_callback_end(H(0));
      usleep(50000);
    }
  }
  static const uint8_t gid[24] = {1, 2, 3};
  // Message stamps the intercept reads from the first 8 bytes: sec 1700000000, nanosec 5.
  static const uint32_t message[4] = {1700000000u, 5u, 0u, 0u};
  for (int i = 0; i < ROUNDS; i++) {
    ros_trace_rcl_init(H(i));
    ros_trace_rcl_node_init(H(i), H(i + 1), "node", "/ns");
    ros_trace_rmw_publisher_init(H(i), gid);
    ros_trace_rcl_publisher_init(H(i), H(i + 1), H(i + 2), "/topic", 10);
    ros_trace_rclcpp_publish(H(i), message);
#if ARMO_TP_rclcpp_intra_publish
    ros_trace_rclcpp_intra_publish(H(i), message);
#endif
    ros_trace_rcl_publish(H(i), message);
#if ARMO_TRACETOOLS_MAJOR >= 8
    ros_trace_rmw_publish(H(i), message, 42 + i);
#else
    ros_trace_rmw_publish(message);
#endif
    ros_trace_rmw_subscription_init(H(i), gid);
    ros_trace_rcl_subscription_init(H(i), H(i + 1), H(i + 2), "/topic", 10);
    ros_trace_rclcpp_subscription_init(H(i), H(i + 1));
    ros_trace_rclcpp_subscription_callback_added(H(i), H(i + 1));
    ros_trace_rmw_take(H(i), message, 42 + i, true);
    ros_trace_rcl_take(message);
    ros_trace_rclcpp_take(message);
    ros_trace_rcl_service_init(H(i), H(i + 1), H(i + 2), "/srv");
    ros_trace_rclcpp_service_callback_added(H(i), H(i + 1));
    ros_trace_rcl_client_init(H(i), H(i + 1), H(i + 2), "/srv");
    ros_trace_rcl_timer_init(H(i), 1000000);
    ros_trace_rclcpp_timer_callback_added(H(i), H(i + 1));
    ros_trace_rclcpp_timer_link_node(H(i), H(i + 1));
    ros_trace_rclcpp_callback_register(H(i + 1), "callback_symbol");
    ros_trace_callback_start(H(i + 1), false);
    ros_trace_callback_end(H(i + 1));
    ros_trace_rclcpp_executor_get_next_ready();
    ros_trace_rclcpp_executor_wait_for_work(100);
    ros_trace_rclcpp_executor_execute(H(i));
#if ARMO_FLUX2
    flux2_trace_channel_init(H(i), 1, 2, "/flux2.v1.seg", 1, 4, 64, -1);
    flux2_trace_claim(H(i), 0, 0, 0);
    flux2_trace_loan(H(i), 0);
    flux2_trace_loan_end(H(i), 0, 0);
    flux2_trace_commit(H(i), 7 + i, 0, 64, 1, 0);
    flux2_trace_take(H(i), 7 + i, 0, 0, 0);
    flux2_trace_release(H(i), 7 + i, 0);
    flux2_trace_refused(H(i), 2);
    flux2_trace_wait(H(i), 1, -1);
    flux2_trace_wake(H(i), 1, 1);
    flux2_trace_callback_start(H(i), 1, H(i + 1));
    flux2_trace_callback_end(H(i), 1, 1, H(i), "/flux2.v1.seg");
    flux2_trace_endpoint_init(H(i + 1), "/node", "/key", "/flux2.v1.seg", 0xA4C, 1, 0);
    flux2_trace_py_gil(0);
    flux2_trace_py_ready(H(i), 7 + i);
    flux2_trace_record_drop(H(i), 0, 3, 0);
    flux2_trace_gpu_wedged(700);
#endif
  }

  const char * once[] = {
    "rcl_init",
    "rcl_node_init",
    "rmw_publisher_init",
    "rcl_publisher_init",
    "rclcpp_publish",
#if ARMO_TP_rclcpp_intra_publish
    "rclcpp_intra_publish",
#endif
    "rcl_publish",
#if ARMO_TRACETOOLS_MAJOR >= 8
    "rmw_publish",
#else
    "rmw_publish_rcl",
#endif
    "rmw_subscription_init",
    "rcl_subscription_init",
    "rclcpp_subscription_init",
    "rclcpp_subscription_callback_added",
    "rmw_take",
    "rcl_take",
    "rclcpp_take",
    "rcl_service_init",
    "rclcpp_service_callback_added",
    "rcl_client_init",
    "rcl_timer_init",
    "rclcpp_timer_callback_added",
    "rclcpp_timer_link_node",
    "rclcpp_callback_register",
    "callback_start",
    "callback_end",
    "rclcpp_executor_get_next_ready",
    "rclcpp_executor_wait_for_work",
    "rclcpp_executor_execute",
  };
  for (size_t i = 0; i < sizeof once / sizeof once[0]; i++) {
    char name[96];
    snprintf(name, sizeof name, "armo:%s", once[i]);
    expect(name, 1);
  }
  // One callback_code after each callback_added.
  expect("armo:callback_code", 3);
#if ARMO_FLUX2
  const char * flux2[] = {
    "channel_init",  "claim",   "loan",     "loan_end",    "commit",         "take",
    "release",       "refused", "wait",     "wake",        "callback_start", "callback_end",
    "endpoint_init", "py_gil",  "py_ready", "record_drop", "gpu_wedged",
  };
  for (size_t i = 0; i < sizeof flux2 / sizeof flux2[0]; i++) {
    char name[96];
    snprintf(name, sizeof name, "flux2:%s", flux2[i]);
    expect(name, 1);
  }
#endif
  return 0;
}
