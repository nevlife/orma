// rcl_wait and rcl_timer_fini of librcl.so (design 2.1.2). Not trace functions: the original
// must run, so a caught call never calls a NULL original. In a Python process without orma_rclpy
// librcl is RTLD_LOCAL and RTLD_NEXT finds nothing; orma_resolve then asks the loaded librcl.so.
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"

#include <stdint.h>

#define RCL "librcl.so"
#define RCL_RET_ERROR 1

typedef int32_t rcl_ret_t;
typedef struct rcl_wait_set_s rcl_wait_set_t;
typedef struct rcl_timer_s rcl_timer_t;

ORMA_EXPORT rcl_ret_t rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout);
ORMA_EXPORT rcl_ret_t rcl_timer_fini(rcl_timer_t * timer);

rcl_ret_t rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout)
{
  rcl_ret_t (*next)(rcl_wait_set_t *, int64_t) = ORMA_NEXT(rcl_wait, RCL, ORMA_EV_RCL_WAIT_ENTER);
  orma_ev_rcl_wait_enter enter = {.wait_set = ORMA_U64(wait_set), .timeout = timeout};
  orma_emit_rcl_wait_enter(&enter);
  rcl_ret_t ret = next != NULL ? next(wait_set, timeout) : RCL_RET_ERROR;
  orma_ev_rcl_wait_exit exit = {.wait_set = ORMA_U64(wait_set), .ret = ret};
  orma_emit_rcl_wait_exit(&exit);
  return ret;
}

rcl_ret_t rcl_timer_fini(rcl_timer_t * timer)
{
  rcl_ret_t (*next)(rcl_timer_t *) = ORMA_NEXT(rcl_timer_fini, RCL, ORMA_EV_RCL_TIMER_FINI);
  orma_ev_handle ev = {.handle = ORMA_U64(timer)};
  orma_emit_rcl_timer_fini(&ev);
  return next != NULL ? next(timer) : RCL_RET_ERROR;
}
