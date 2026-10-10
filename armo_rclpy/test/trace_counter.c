// Loaded with LD_PRELOAD in front of libtracetools.so and librcl.so. Logs each call with its
// pointer arguments, then forwards it to the next definition.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

enum event {
  RCL_NODE_INIT,
  RCL_SUBSCRIPTION_INIT,
  RCL_SERVICE_INIT,
  RCL_CLIENT_INIT,
  RCL_TIMER_INIT,
  RCLCPP_SUBSCRIPTION_INIT,
  RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
  RCLCPP_SERVICE_CALLBACK_ADDED,
  RCLCPP_TIMER_CALLBACK_ADDED,
  RCLCPP_TIMER_LINK_NODE,
  RCLCPP_CALLBACK_REGISTER,
  CALLBACK_START,
  CALLBACK_END,
  RCL_WAIT,
};

struct record {
  int32_t event;
  int32_t flag;
  uint64_t a;
  uint64_t b;
  char text[96];
};

#define CAPACITY 65536

static struct record records[CAPACITY];
static uint64_t count;

__attribute__((visibility("default"))) uint64_t trace_counter_read(const struct record ** out)
{
  *out = records;
  return __atomic_load_n(&count, __ATOMIC_ACQUIRE);
}

static void log_event(int event, const void * a, const void * b, int flag, const char * text)
{
  uint64_t i = __atomic_fetch_add(&count, 1, __ATOMIC_RELAXED);
  if (i >= CAPACITY) return;
  records[i].event = event;
  records[i].flag = flag;
  records[i].a = (uint64_t)(uintptr_t)a;
  records[i].b = (uint64_t)(uintptr_t)b;
  if (text != NULL) strncpy(records[i].text, text, sizeof(records[i].text) - 1);
}

#define FORWARD(fn, ...)                       \
  do {                                         \
    static __typeof__(&fn) next;               \
    if (next == NULL) next = dlsym(RTLD_NEXT, #fn); \
    next(__VA_ARGS__);                         \
  } while (0)

void ros_trace_rcl_node_init(
  const void * node_handle, const void * rmw_handle, const char * node_name,
  const char * node_namespace)
{
  log_event(RCL_NODE_INIT, node_handle, NULL, 0, node_name);
  FORWARD(ros_trace_rcl_node_init, node_handle, rmw_handle, node_name, node_namespace);
}

void ros_trace_rcl_subscription_init(
  const void * subscription_handle, const void * node_handle, const void * rmw_subscription_handle,
  const char * topic_name, const size_t queue_depth)
{
  log_event(RCL_SUBSCRIPTION_INIT, subscription_handle, node_handle, 0, topic_name);
  FORWARD(
    ros_trace_rcl_subscription_init, subscription_handle, node_handle, rmw_subscription_handle,
    topic_name, queue_depth);
}

void ros_trace_rcl_service_init(
  const void * service_handle, const void * node_handle, const void * rmw_service_handle,
  const char * service_name)
{
  log_event(RCL_SERVICE_INIT, service_handle, node_handle, 0, service_name);
  FORWARD(ros_trace_rcl_service_init, service_handle, node_handle, rmw_service_handle, service_name);
}

void ros_trace_rcl_client_init(
  const void * client_handle, const void * node_handle, const void * rmw_client_handle,
  const char * service_name)
{
  log_event(RCL_CLIENT_INIT, client_handle, node_handle, 0, service_name);
  FORWARD(ros_trace_rcl_client_init, client_handle, node_handle, rmw_client_handle, service_name);
}

void ros_trace_rcl_timer_init(const void * timer_handle, int64_t period)
{
  log_event(RCL_TIMER_INIT, timer_handle, NULL, 0, NULL);
  FORWARD(ros_trace_rcl_timer_init, timer_handle, period);
}

void ros_trace_rclcpp_subscription_init(const void * subscription_handle, const void * subscription)
{
  log_event(RCLCPP_SUBSCRIPTION_INIT, subscription_handle, subscription, 0, NULL);
  FORWARD(ros_trace_rclcpp_subscription_init, subscription_handle, subscription);
}

void ros_trace_rclcpp_subscription_callback_added(const void * subscription, const void * callback)
{
  log_event(RCLCPP_SUBSCRIPTION_CALLBACK_ADDED, subscription, callback, 0, NULL);
  FORWARD(ros_trace_rclcpp_subscription_callback_added, subscription, callback);
}

void ros_trace_rclcpp_service_callback_added(const void * service_handle, const void * callback)
{
  log_event(RCLCPP_SERVICE_CALLBACK_ADDED, service_handle, callback, 0, NULL);
  FORWARD(ros_trace_rclcpp_service_callback_added, service_handle, callback);
}

void ros_trace_rclcpp_timer_callback_added(const void * timer_handle, const void * callback)
{
  log_event(RCLCPP_TIMER_CALLBACK_ADDED, timer_handle, callback, 0, NULL);
  FORWARD(ros_trace_rclcpp_timer_callback_added, timer_handle, callback);
}

void ros_trace_rclcpp_timer_link_node(const void * timer_handle, const void * node_handle)
{
  log_event(RCLCPP_TIMER_LINK_NODE, timer_handle, node_handle, 0, NULL);
  FORWARD(ros_trace_rclcpp_timer_link_node, timer_handle, node_handle);
}

void ros_trace_rclcpp_callback_register(const void * callback, const char * function_symbol)
{
  log_event(RCLCPP_CALLBACK_REGISTER, callback, NULL, 0, function_symbol);
  FORWARD(ros_trace_rclcpp_callback_register, callback, function_symbol);
}

void ros_trace_callback_start(const void * callback, const bool is_intra_process)
{
  log_event(CALLBACK_START, callback, NULL, is_intra_process, NULL);
  FORWARD(ros_trace_callback_start, callback, is_intra_process);
}

void ros_trace_callback_end(const void * callback)
{
  log_event(CALLBACK_END, callback, NULL, 0, NULL);
  FORWARD(ros_trace_callback_end, callback);
}

// rcl_ret_t rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout)
int32_t rcl_wait(void * wait_set, int64_t timeout)
{
  static __typeof__(&rcl_wait) next;
  if (next == NULL) next = dlsym(RTLD_NEXT, "rcl_wait");
  log_event(RCL_WAIT, wait_set, NULL, 0, NULL);
  return next(wait_set, timeout);
}
