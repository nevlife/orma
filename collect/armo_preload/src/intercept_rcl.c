// rcl_wait and rcl_timer_fini of librcl.so (design 2.1.2). Not trace functions: the original
// must run, so a caught call never calls a NULL original. In a Python process without armo_rclpy
// librcl is RTLD_LOCAL and RTLD_NEXT finds nothing; armo_resolve then asks the loaded librcl.so.
#include "armo_preload/armo_emit.h"
#include "armo_preload/intercept.h"

#include <stdint.h>

#define RCL "librcl.so"
#define RCL_RET_ERROR 1

typedef int32_t rcl_ret_t;
typedef struct rcl_wait_set_s rcl_wait_set_t;
typedef struct rcl_timer_s rcl_timer_t;

ARMO_EXPORT rcl_ret_t rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout);
ARMO_EXPORT rcl_ret_t rcl_timer_fini(rcl_timer_t * timer);

rcl_ret_t rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout)
{
  rcl_ret_t (*next)(rcl_wait_set_t *, int64_t) = ARMO_NEXT(rcl_wait, RCL, ARMO_EV_RCL_WAIT_ENTER);
  armo_ev_rcl_wait_enter enter = {.wait_set = ARMO_U64(wait_set), .timeout = timeout};
  armo_emit_rcl_wait_enter(&enter);
  rcl_ret_t ret = next != NULL ? next(wait_set, timeout) : RCL_RET_ERROR;
  armo_ev_rcl_wait_exit exit = {.wait_set = ARMO_U64(wait_set), .ret = ret};
  armo_emit_rcl_wait_exit(&exit);
  return ret;
}

rcl_ret_t rcl_timer_fini(rcl_timer_t * timer)
{
  rcl_ret_t (*next)(rcl_timer_t *) = ARMO_NEXT(rcl_timer_fini, RCL, ARMO_EV_RCL_TIMER_FINI);
  armo_ev_handle ev = {.handle = ARMO_U64(timer)};
  armo_emit_rcl_timer_fini(&ev);
  return next != NULL ? next(timer) : RCL_RET_ERROR;
}
