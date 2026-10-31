// Calls every ros_trace_* this distro's libtracetools exports and, when built with flux, every
// flux_trace_* function ROUNDS times with known arguments, and prints for each orma event the
// count it should cause: "<provider:event> <count>". lttng_events.py runs it under
// LD_PRELOAD=liborma_lttng.so in an LTTng session and compares.
//
//     lttng_caller CUDA DDS    also spans, and calls into the CUDA and DDS stubs of orma_preload's
//                              tests, timed after the caller turns both on in its .lttng file
//     lttng_caller burst N     N callback_start, callback_end pairs, busy 6 ms of every 10 ms
//     (lifecycle.py) lttng_caller tick        a pair every 50 ms until killed
#define _GNU_SOURCE
#include "orma_preload/cuda_api.h"
#include "orma_preload/orma_shm.h"
#include "orma_preload/orma_tracepoints.h"
#include "orma_span/span.h"
#include "tracetools/tracetools.h"

#include <dlfcn.h>
#include <fcntl.h>
#include <glob.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#if ORMA_FLUX
#include "flux/trace.h"
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

// The options word of this process's .lttng file, which its first callback made.
static uint32_t * own_options(void)
{
  char pattern[64];
  snprintf(pattern, sizeof pattern, "/dev/shm/orma.%d.*.lttng", (int)getpid());
  glob_t g;
  if (glob(pattern, 0, NULL, &g) != 0) {
    exit(1);
  }
  int fd = open(g.gl_pathv[0], O_RDWR | O_CLOEXEC);
  globfree(&g);
  orma_shm_header * h = mmap(NULL, ORMA_SHM_HEADER_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (h == MAP_FAILED) {
    exit(1);
  }
  return &h->options;
}

typedef CUresult (*htod_fn)(CUdeviceptr, const void *, size_t);
typedef CUresult (*sync_fn)(CUstream);
typedef uintptr_t (*write_params_fn)(uintptr_t, uintptr_t, uintptr_t);

// A span pair, a host-to-device copy and a per-thread stream sync through cuGetProcAddress as
// cudart takes it, and a Cyclone and a Fast DDS write each round; one copy and one write before
// the options are on, which no event follows.
static void collect(const char * cuda_stub, const char * dds_stub)
{
  static const char outer[] = "outer";
  static const char inner[] = "inner";
  void * cuda = dlopen(cuda_stub, RTLD_NOW | RTLD_LOCAL);
  void * dds = dlopen(dds_stub, RTLD_NOW | RTLD_GLOBAL);
  if (cuda == NULL || dds == NULL) {
    exit(1);
  }
  orma_cu_get_proc_address gpa_v1 = (orma_cu_get_proc_address)dlsym(cuda, "cuGetProcAddress");
  orma_cu_get_proc_address_v2 gpa = NULL;
  gpa_v1("cuGetProcAddress", (void **)&gpa, 12000, 0);
  htod_fn htod = NULL;
  sync_fn sync = NULL;
  gpa("cuMemcpyHtoD", (void **)&htod, 13000, 0, NULL);
  gpa("cuStreamSynchronize", (void **)&sync, 13000, ORMA_CU_PER_THREAD, NULL);
  int32_t (*dds_write)(int32_t, const void *) =
    (int32_t(*)(int32_t, const void *))dlsym(RTLD_DEFAULT, "dds_write");
  write_params_fn fastdds = (write_params_fn)dlsym(
    RTLD_DEFAULT, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPvRNS_8fastrtps4rtps11WriteParamsE");
  char buf[64] = {0};
  htod(0, buf, sizeof buf);
  dds_write(0x1234, buf);
  *own_options() = ORMA_OPTION_GPU | ORMA_OPTION_DDS;
  for (int i = 0; i < ROUNDS; i++) {
    orma_span_begin(outer);
    orma_span_begin(inner);
    htod(0, buf, sizeof buf);
    sync(NULL);
    orma_span_end(inner);
    dds_write(0x1234, buf);
    fastdds(0xBEEF, 0, 0);
    orma_span_end(outer);
  }
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
#if ORMA_TP_rclcpp_intra_publish
    ros_trace_rclcpp_intra_publish(H(i), message);
#endif
    ros_trace_rcl_publish(H(i), message);
#if ORMA_TRACETOOLS_MAJOR >= 8
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
#if ORMA_FLUX
    flux_trace_channel_init(H(i), 1, 2, "/flux.v1.seg", 1, 4, 64, -1);
    flux_trace_claim(H(i), 0, 0, 0);
    flux_trace_loan(H(i), 0);
    flux_trace_loan_end(H(i), 0, 0);
    flux_trace_commit(H(i), 7 + i, 0, 64, 1, 0);
    flux_trace_take(H(i), 7 + i, 0, 0, 0);
    flux_trace_release(H(i), 7 + i, 0);
    flux_trace_refused(H(i), 2);
    flux_trace_wait(H(i), 1, -1);
    flux_trace_wake(H(i), 1, 1);
    flux_trace_callback_start(H(i), 1, H(i + 1));
    flux_trace_callback_end(H(i), 1, 1, H(i), "/flux.v1.seg");
    flux_trace_endpoint_init(H(i + 1), "/node", "/key", "/flux.v1.seg", 0xA4C, 1, 0);
    flux_trace_py_gil(0);
    flux_trace_py_ready(H(i), 7 + i);
    flux_trace_record_drop(H(i), 0, 3, 0);
    flux_trace_gpu_wedged(700);
#endif
  }

  const char * once[] = {
    "rcl_init",
    "rcl_node_init",
    "rmw_publisher_init",
    "rcl_publisher_init",
    "rclcpp_publish",
#if ORMA_TP_rclcpp_intra_publish
    "rclcpp_intra_publish",
#endif
    "rcl_publish",
#if ORMA_TRACETOOLS_MAJOR >= 8
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
    snprintf(name, sizeof name, "orma:%s", once[i]);
    expect(name, 1);
  }
  // One callback_code after each callback_added.
  expect("orma:callback_code", 3);
  if (argc == 3) {
    collect(argv[1], argv[2]);
    expect("orma:span_begin", 2);
    expect("orma:span_end", 2);
    expect("orma:cuda_call", 2);
    expect("orma:dds_write", 2);
  }
#if ORMA_FLUX
  const char * flux[] = {
    "channel_init",  "claim",   "loan",     "loan_end",    "commit",         "take",
    "release",       "refused", "wait",     "wake",        "callback_start", "callback_end",
    "endpoint_init", "py_gil",  "py_ready", "record_drop", "gpu_wedged",
  };
  for (size_t i = 0; i < sizeof flux / sizeof flux[0]; i++) {
    char name[96];
    snprintf(name, sizeof name, "flux:%s", flux[i]);
    expect(name, 1);
  }
#endif
  return 0;
}
