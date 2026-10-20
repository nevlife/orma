// rclcpp C++ functions without a trace function (design 2.1.2). orma_rclcpp_symbols.h, generated
// from this distro's librclcpp.so, lists each caught symbol with its argument count and which
// argument is the object (cmake/gen_symbols.py). Every argument is a pointer, an integer or a
// shared_ptr passed by invisible reference, so integer parameters forward the call unchanged.
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"
#include "orma_preload/orma_rclcpp_symbols.h"

#include <stdint.h>

#define RCLCPP "librclcpp.so"

enum arg_kind {
  ARG_NONE,
  ARG_VALUE,
  ARG_DEREF,
  ARG_IPM,
};

static void emit_cxx(uint16_t event, enum arg_kind kind, int index, const uintptr_t * a)
{
  if (kind == ARG_IPM) {
    orma_ev_rclcpp_ipm_insert_sub_id_for_pub ev = {
      .ipm = a[0], .sub_id = a[1], .pub_id = a[2], .use_take_shared_method = a[3] & 1};
    orma_emit_rclcpp_ipm_insert_sub_id_for_pub(&ev);
    return;
  }
  orma_ev_rclcpp_object ev = {.object = a[0]};
  if (kind == ARG_VALUE) ev.arg = a[index];
  if (kind == ARG_DEREF) ev.arg = *(const uint64_t *)a[index];
  switch (event) {
    case ORMA_EV_RCLCPP_PUBLISHER_BASE:
      orma_emit_rclcpp_publisher_base(&ev);
      break;
    case ORMA_EV_RCLCPP_PUBLISHER_SETUP_INTRA:
      orma_emit_rclcpp_publisher_setup_intra(&ev);
      break;
    case ORMA_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA:
      orma_emit_rclcpp_subscription_setup_intra(&ev);
      break;
    case ORMA_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION:
      orma_emit_rclcpp_group_add_subscription(&ev);
      break;
    case ORMA_EV_RCLCPP_GROUP_ADD_TIMER:
      orma_emit_rclcpp_group_add_timer(&ev);
      break;
    case ORMA_EV_RCLCPP_EXECUTOR:
      orma_emit_rclcpp_executor(&ev);
      break;
    case ORMA_EV_RCLCPP_COLLECTOR:
      orma_emit_rclcpp_collector(&ev);
      break;
    case ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION:
      orma_emit_rclcpp_collector_add_group_to_collection(&ev);
      break;
    case ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP:
      orma_emit_rclcpp_collector_add_group(&ev);
      break;
    case ORMA_EV_RCLCPP_EXECUTOR_ADD_GROUP:
      orma_emit_rclcpp_executor_add_group(&ev);
      break;
  }
}

#define P2 uintptr_t a0, uintptr_t a1
#define P3 P2, uintptr_t a2
#define P4 P3, uintptr_t a3
#define P5 P4, uintptr_t a4
#define P6 P5, uintptr_t a5
#define P7 P6, uintptr_t a6
#define A2 a0, a1
#define A3 A2, a2
#define A4 A3, a3
#define A5 A4, a4
#define A6 A5, a5
#define A7 A6, a6

// Returns the original's x0/rax: a constructor that returns `this` on some ABIs keeps it.
#define CXX_WRAPPER(id, name, event, n, kind, index)              \
  ORMA_EXPORT uintptr_t id(P##n) __asm__(name);                   \
  uintptr_t id(P##n)                                              \
  {                                                               \
    const uintptr_t a[] = {A##n};                                 \
    emit_cxx(event, kind, index, a);                              \
    static void * cache;                                          \
    void * f = __atomic_load_n(&cache, __ATOMIC_RELAXED);         \
    if (f == NULL) f = orma_resolve(&cache, name, RCLCPP, event); \
    return ((uintptr_t(*)(P##n))f)(A##n);                         \
  }

ORMA_RCLCPP_SYMBOLS(CXX_WRAPPER)

// The rclcpp points this build found no symbol for (design 2.1.2), marked once.
#define MARK_UNBOUND(event) orma_backend_missing(event);

void orma_rclcpp_report_unbound(void)
{
  ORMA_RCLCPP_UNBOUND(MARK_UNBOUND)
}
