// The backend of libarmo_preload.so: the shared memory file of design 13 (slots.c), made on the
// process's first event, with each thread's aggregation table and ring.
#define _GNU_SOURCE
#include "armo_preload/armo_emit.h"
#include "armo_preload/slots.h"

#include <string.h>
#include <time.h>

// Fixed in this backend (only the slot count comes from ARMO_THREADS), so the compiler turns the
// modulo of a table probe and a ring position into masks.
#define TABLE_ENTRIES ARMO_TABLE_ENTRIES_DEFAULT
#define RING_BYTES ARMO_RING_BYTES_DEFAULT
#define LOAD ARMO_LOAD
#define STORE ARMO_STORE
#define BUMP ARMO_BUMP

const struct armo_layout armo_layout = {
  .backend = ARMO_BACKEND_SHM,
  .table_entries = TABLE_ENTRIES,
  .ring_bytes = RING_BYTES,
  .suffix = "",
};

static uint64_t now(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

static uint32_t bucket(uint64_t v)
{
  uint32_t b = v == 0 ? 0 : 63 - (uint32_t)__builtin_clzll(v);
  return b < ARMO_BUCKETS ? b : ARMO_BUCKETS - 1;
}

static void table_add(
  struct armo_thread * t, uint16_t kind, uint16_t src, uint32_t entry, uint64_t key, uint64_t value)
{
  const uint32_t n = TABLE_ENTRIES;
  uint64_t mix = (key ^ (key >> 17) ^ ((uint64_t)entry << 7) ^ kind ^ ((uint64_t)src << 3)) *
                 0x9E3779B97F4A7C15u;
  uint32_t i = (uint32_t)((mix >> 32) % n);
  for (uint32_t probe = 0; probe < n; probe++, i = i + 1 == n ? 0 : i + 1) {
    armo_table_entry * e = (armo_table_entry *)(t->slot + 1) + i;
    if (e->kind == 0) {
      e->src = src;
      e->entry = entry;
      STORE(&e->key, key);
      __atomic_store_n(&e->kind, kind, __ATOMIC_RELEASE);
    } else if (e->kind != kind || e->src != src || e->entry != entry || e->key != key) {
      continue;
    }
    BUMP(&e->count);
    STORE(&e->sum, e->sum + value);
    // The reader swaps max to 0 each window, so it is the one field updated atomically.
    uint64_t max = LOAD(&e->max);
    while (value > max && !__atomic_compare_exchange_n(
                            &e->max, &max, value, true, __ATOMIC_RELAXED, __ATOMIC_RELAXED)) {
    }
    BUMP(&e->hist[bucket(value)]);
    return;
  }
  BUMP(&t->slot->table_overflow);
}

static uint32_t round8(uint32_t v)
{
  return (v + ARMO_RECORD_ALIGN - 1) & ~(ARMO_RECORD_ALIGN - 1);
}

static void ring_put(
  struct armo_thread * t, uint16_t event, uint64_t ts, const void * payload, uint32_t size,
  const char * const * strs, uint32_t n_strs)
{
  uint32_t lens[3];
  uint32_t flags = 0;
  uint32_t len = (uint32_t)sizeof(armo_record) + size;
  for (uint32_t i = 0; i < n_strs; i++) {
    size_t l = strnlen(strs[i] != NULL ? strs[i] : "", ARMO_STRING_MAX);
    if (l == ARMO_STRING_MAX) {
      l = ARMO_STRING_MAX - 1;
      flags |= ARMO_RECORD_TRUNCATED;
    }
    lens[i] = (uint32_t)l;
    len += lens[i] + 1;
  }
  len = round8(len);

  armo_thread_slot * s = t->slot;
  uint64_t head = LOAD(&s->ring_head);
  uint64_t tail = __atomic_load_n(&s->ring_tail, __ATOMIC_ACQUIRE);
  uint64_t pos = head % RING_BYTES;
  uint64_t to_end = RING_BYTES - pos;
  uint64_t need = len <= to_end ? len : to_end + len;
  if (RING_BYTES - (head - tail) < need) {
    BUMP(&s->ring_drops);
    return;
  }
  uint8_t * ring = (uint8_t *)s + armo_ring_offset(TABLE_ENTRIES);
  uint8_t * at = ring + pos;
  if (len > to_end) {
    armo_record pad = {.event = ARMO_EV_PAD, .len = (uint16_t)to_end, .t_mono_ns = ts};
    memcpy(at, &pad, to_end < sizeof pad ? to_end : sizeof pad);
    at = ring;
  }
  armo_record r = {.event = event, .len = (uint16_t)len, .flags = flags, .t_mono_ns = ts};
  uint8_t * p = at;
  memcpy(p, &r, sizeof r);
  p += sizeof r;
  memcpy(p, payload, size);
  p += size;
  for (uint32_t i = 0; i < n_strs; i++) {
    memcpy(p, strs[i] != NULL ? strs[i] : "", lens[i]);
    p[lens[i]] = '\0';
    p += lens[i] + 1;
  }
  memset(p, 0, (size_t)(at + len - p));
  __atomic_store_n(&s->ring_head, head + need, __ATOMIC_RELEASE);
}

static bool armed(void)
{
  return LOAD(&armo_header->armed) != 0;
}

// What names and links entities. It reaches the ring before the file is armed, so that a reader
// that arms it later still knows them; armo_common's catalog::init is the same list.
static bool init_event(uint16_t event)
{
  switch (event) {
    case ARMO_EV_RCL_INIT:
    case ARMO_EV_RCL_NODE_INIT:
    case ARMO_EV_RMW_PUBLISHER_INIT:
    case ARMO_EV_RCL_PUBLISHER_INIT:
    case ARMO_EV_RMW_SUBSCRIPTION_INIT:
    case ARMO_EV_RCL_SUBSCRIPTION_INIT:
    case ARMO_EV_RCLCPP_SUBSCRIPTION_INIT:
    case ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED:
    case ARMO_EV_RCL_SERVICE_INIT:
    case ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED:
    case ARMO_EV_RCL_CLIENT_INIT:
    case ARMO_EV_RCL_TIMER_INIT:
    case ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED:
    case ARMO_EV_RCLCPP_TIMER_LINK_NODE:
    case ARMO_EV_RCLCPP_CALLBACK_REGISTER:
    case ARMO_EV_RMW_CLIENT_INIT:
    case ARMO_EV_RCL_TIMER_FINI:
    case ARMO_EV_RCLCPP_PUBLISHER_BASE:
    case ARMO_EV_RCLCPP_PUBLISHER_SETUP_INTRA:
    case ARMO_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA:
    case ARMO_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB:
    case ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION:
    case ARMO_EV_RCLCPP_GROUP_ADD_TIMER:
    case ARMO_EV_RCLCPP_EXECUTOR:
    case ARMO_EV_RCLCPP_COLLECTOR:
    case ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION:
    case ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP:
    case ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP:
    case ARMO_EV_RCLCPP_CALLBACK_OWNER:
    case ARMO_EV_CALLBACK_CODE:
    case ARMO_EV_FLUX2_CHANNEL_INIT:
    case ARMO_EV_FLUX2_ENDPOINT_INIT:
    case ARMO_EV_FLUX2_GPU_WEDGED:
      return true;
    default:
      return false;
  }
}

static void record(
  uint16_t event, const void * payload, uint32_t size, const char * const * strs, uint32_t n_strs)
{
  struct armo_thread * t = armo_thread();
  if (t != NULL && (armed() || init_event(event))) {
    ring_put(t, event, now(), payload, size, strs, n_strs);
  }
}

static inline __attribute__((always_inline)) void callback_start(
  uint16_t event, const void * ev, uint32_t size, uint16_t src, uint64_t key, uint32_t entry,
  uint64_t executor)
{
  struct armo_thread * t = armo_thread();
  if (t == NULL) return;
  uint64_t ts = now();
  const struct armo_wake * w = &t->wake[src];
  if (w->t != 0 && w->depth == t->n + t->untracked && w->executor == executor) {
    table_add(t, ARMO_AGG_DISPATCH, src, entry, key, ts - w->t);
  }
  armo_push(t, src, key, entry, ts);
  if (armed()) ring_put(t, event, ts, ev, size, NULL, 0);
}

// A flux2 pass that delivered nothing is not counted as a callback.
static inline __attribute__((always_inline)) void callback_end(
  uint16_t event, const void * ev, uint32_t size, const char * const * strs, uint32_t n_strs,
  uint16_t src, uint64_t key, uint32_t entry, bool counted)
{
  struct armo_thread * t = armo_thread();
  if (t == NULL) return;
  uint64_t ts = now();
  uint64_t t0;
  if (armo_pop(t, src, key, entry, &t0) && counted) {
    table_add(t, ARMO_AGG_CALLBACK, src, entry, key, ts - t0);
  }
  if (armed()) ring_put(t, event, ts, ev, size, strs, n_strs);
}

static void wait_enter(uint16_t event, const void * ev, uint32_t size, uint16_t src)
{
  struct armo_thread * t = armo_thread();
  if (t == NULL) return;
  uint64_t ts = now();
  t->wait_start[src] = ts;
  t->wake[src].t = 0;
  if (armed()) ring_put(t, event, ts, ev, size, NULL, 0);
}

static void wait_exit(
  uint16_t event, const void * ev, uint32_t size, uint16_t src, uint64_t key, uint64_t executor)
{
  struct armo_thread * t = armo_thread();
  if (t == NULL) return;
  uint64_t ts = now();
  if (t->wait_start[src] != 0) table_add(t, ARMO_AGG_WAIT, src, 0, key, ts - t->wait_start[src]);
  t->wait_start[src] = 0;
  t->wake[src] = (struct armo_wake){.t = ts, .executor = executor, .depth = t->n + t->untracked};
  if (armed()) ring_put(t, event, ts, ev, size, NULL, 0);
}

void armo_emit_callback_start(const armo_ev_callback_start * ev)
{
  callback_start(ARMO_EV_CALLBACK_START, ev, sizeof *ev, ARMO_SRC_ROS, ev->callback, 0, 0);
}

void armo_emit_callback_end(const armo_ev_handle * ev)
{
  callback_end(ARMO_EV_CALLBACK_END, ev, sizeof *ev, NULL, 0, ARMO_SRC_ROS, ev->handle, 0, true);
}

void armo_emit_flux2_callback_start(const armo_ev_flux2_callback_start * ev)
{
  callback_start(
    ARMO_EV_FLUX2_CALLBACK_START, ev, sizeof *ev, ARMO_SRC_FLUX2, ev->executor, ev->entry,
    ev->executor);
}

void armo_emit_flux2_callback_end(const armo_ev_flux2_callback_end * ev, const char * segment)
{
  const char * strs[] = {segment};
  callback_end(
    ARMO_EV_FLUX2_CALLBACK_END, ev, sizeof *ev, strs, 1, ARMO_SRC_FLUX2, ev->executor, ev->entry,
    ev->delivered > 0);
}

void armo_emit_rcl_wait_enter(const armo_ev_rcl_wait_enter * ev)
{
  wait_enter(ARMO_EV_RCL_WAIT_ENTER, ev, sizeof *ev, ARMO_SRC_ROS);
}

void armo_emit_rcl_wait_exit(const armo_ev_rcl_wait_exit * ev)
{
  wait_exit(ARMO_EV_RCL_WAIT_EXIT, ev, sizeof *ev, ARMO_SRC_ROS, ev->wait_set, 0);
}

void armo_emit_flux2_wait(const armo_ev_flux2_wait * ev)
{
  wait_enter(ARMO_EV_FLUX2_WAIT, ev, sizeof *ev, ARMO_SRC_FLUX2);
}

void armo_emit_flux2_wake(const armo_ev_flux2_wake * ev)
{
  wait_exit(ARMO_EV_FLUX2_WAKE, ev, sizeof *ev, ARMO_SRC_FLUX2, ev->executor, ev->executor);
}

#define PLAIN(name, event, type)            \
  void armo_emit_##name(const type * ev)    \
  {                                         \
    record(event, ev, sizeof *ev, NULL, 0); \
  }

#define STRINGS(name, event, type, n, ...)            \
  void armo_emit_##name(const type * ev, __VA_ARGS__) \
  {                                                   \
    const char * strs[] = {STRING_ARGS_##n};          \
    record(event, ev, sizeof *ev, strs, n);           \
  }
#define STRING_ARGS_1 s0
#define STRING_ARGS_2 s0, s1
#define STRING_ARGS_3 s0, s1, s2

PLAIN(rcl_init, ARMO_EV_RCL_INIT, armo_ev_rcl_init)
STRINGS(
  rcl_node_init, ARMO_EV_RCL_NODE_INIT, armo_ev_rcl_node_init, 2, const char * s0, const char * s1)
PLAIN(rmw_publisher_init, ARMO_EV_RMW_PUBLISHER_INIT, armo_ev_rmw_endpoint_init)
STRINGS(rcl_publisher_init, ARMO_EV_RCL_PUBLISHER_INIT, armo_ev_rcl_topic_init, 1, const char * s0)
PLAIN(rclcpp_publish, ARMO_EV_RCLCPP_PUBLISH, armo_ev_publish)
PLAIN(rclcpp_intra_publish, ARMO_EV_RCLCPP_INTRA_PUBLISH, armo_ev_publish)
PLAIN(rcl_publish, ARMO_EV_RCL_PUBLISH, armo_ev_publish)
PLAIN(rmw_publish, ARMO_EV_RMW_PUBLISH, armo_ev_rmw_publish)
PLAIN(rmw_publish_rcl, ARMO_EV_RMW_PUBLISH_RCL, armo_ev_rmw_publish_rcl)
PLAIN(rmw_subscription_init, ARMO_EV_RMW_SUBSCRIPTION_INIT, armo_ev_rmw_endpoint_init)
STRINGS(
  rcl_subscription_init, ARMO_EV_RCL_SUBSCRIPTION_INIT, armo_ev_rcl_topic_init, 1, const char * s0)
PLAIN(rclcpp_subscription_init, ARMO_EV_RCLCPP_SUBSCRIPTION_INIT, armo_ev_rclcpp_subscription_init)
PLAIN(
  rclcpp_subscription_callback_added, ARMO_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
  armo_ev_callback_added)
PLAIN(rmw_take, ARMO_EV_RMW_TAKE, armo_ev_rmw_take)
PLAIN(rcl_take, ARMO_EV_RCL_TAKE, armo_ev_take)
PLAIN(rclcpp_take, ARMO_EV_RCLCPP_TAKE, armo_ev_take)
STRINGS(rcl_service_init, ARMO_EV_RCL_SERVICE_INIT, armo_ev_rcl_service_init, 1, const char * s0)
PLAIN(rclcpp_service_callback_added, ARMO_EV_RCLCPP_SERVICE_CALLBACK_ADDED, armo_ev_callback_added)
STRINGS(rcl_client_init, ARMO_EV_RCL_CLIENT_INIT, armo_ev_rcl_service_init, 1, const char * s0)
PLAIN(rcl_timer_init, ARMO_EV_RCL_TIMER_INIT, armo_ev_rcl_timer_init)
PLAIN(rclcpp_timer_callback_added, ARMO_EV_RCLCPP_TIMER_CALLBACK_ADDED, armo_ev_callback_added)
PLAIN(rclcpp_timer_link_node, ARMO_EV_RCLCPP_TIMER_LINK_NODE, armo_ev_rclcpp_timer_link_node)
STRINGS(
  rclcpp_callback_register, ARMO_EV_RCLCPP_CALLBACK_REGISTER, armo_ev_rclcpp_callback_register, 1,
  const char * s0)
PLAIN(
  rclcpp_executor_wait_for_work, ARMO_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK,
  armo_ev_rclcpp_executor_wait_for_work)
PLAIN(rclcpp_executor_execute, ARMO_EV_RCLCPP_EXECUTOR_EXECUTE, armo_ev_handle)
PLAIN(rmw_client_init, ARMO_EV_RMW_CLIENT_INIT, armo_ev_rmw_endpoint_init)
PLAIN(rmw_send_request, ARMO_EV_RMW_SEND_REQUEST, armo_ev_rmw_send_request)
PLAIN(rmw_take_request, ARMO_EV_RMW_TAKE_REQUEST, armo_ev_rmw_take_request)
PLAIN(rmw_send_response, ARMO_EV_RMW_SEND_RESPONSE, armo_ev_rmw_send_response)
PLAIN(rmw_take_response, ARMO_EV_RMW_TAKE_RESPONSE, armo_ev_rmw_take_response)
PLAIN(rcl_timer_fini, ARMO_EV_RCL_TIMER_FINI, armo_ev_handle)
PLAIN(rclcpp_publisher_base, ARMO_EV_RCLCPP_PUBLISHER_BASE, armo_ev_rclcpp_object)
PLAIN(rclcpp_publisher_setup_intra, ARMO_EV_RCLCPP_PUBLISHER_SETUP_INTRA, armo_ev_rclcpp_object)
PLAIN(
  rclcpp_subscription_setup_intra, ARMO_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA, armo_ev_rclcpp_object)
PLAIN(
  rclcpp_ipm_insert_sub_id_for_pub, ARMO_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB,
  armo_ev_rclcpp_ipm_insert_sub_id_for_pub)
PLAIN(rclcpp_group_add_subscription, ARMO_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION, armo_ev_rclcpp_object)
PLAIN(rclcpp_group_add_timer, ARMO_EV_RCLCPP_GROUP_ADD_TIMER, armo_ev_rclcpp_object)
PLAIN(rclcpp_executor, ARMO_EV_RCLCPP_EXECUTOR, armo_ev_rclcpp_object)
PLAIN(rclcpp_collector, ARMO_EV_RCLCPP_COLLECTOR, armo_ev_rclcpp_object)
PLAIN(
  rclcpp_collector_add_group_to_collection, ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION,
  armo_ev_rclcpp_object)
PLAIN(rclcpp_collector_add_group, ARMO_EV_RCLCPP_COLLECTOR_ADD_GROUP, armo_ev_rclcpp_object)
PLAIN(rclcpp_executor_add_group, ARMO_EV_RCLCPP_EXECUTOR_ADD_GROUP, armo_ev_rclcpp_object)
PLAIN(callback_code, ARMO_EV_CALLBACK_CODE, armo_ev_callback_code)
STRINGS(
  flux2_channel_init, ARMO_EV_FLUX2_CHANNEL_INIT, armo_ev_flux2_channel_init, 1, const char * s0)
PLAIN(flux2_claim, ARMO_EV_FLUX2_CLAIM, armo_ev_flux2_claim)
PLAIN(flux2_loan, ARMO_EV_FLUX2_LOAN, armo_ev_flux2_loan)
PLAIN(flux2_loan_end, ARMO_EV_FLUX2_LOAN_END, armo_ev_flux2_loan_end)
PLAIN(flux2_commit, ARMO_EV_FLUX2_COMMIT, armo_ev_flux2_commit)
PLAIN(flux2_take, ARMO_EV_FLUX2_TAKE, armo_ev_flux2_take)
PLAIN(flux2_release, ARMO_EV_FLUX2_RELEASE, armo_ev_flux2_release)
PLAIN(flux2_refused, ARMO_EV_FLUX2_REFUSED, armo_ev_flux2_refused)
STRINGS(
  flux2_endpoint_init, ARMO_EV_FLUX2_ENDPOINT_INIT, armo_ev_flux2_endpoint_init, 3, const char * s0,
  const char * s1, const char * s2)
PLAIN(flux2_py_gil, ARMO_EV_FLUX2_PY_GIL, armo_ev_flux2_py_gil)
PLAIN(flux2_py_ready, ARMO_EV_FLUX2_PY_READY, armo_ev_flux2_py_ready)
PLAIN(flux2_record_drop, ARMO_EV_FLUX2_RECORD_DROP, armo_ev_flux2_record_drop)
PLAIN(flux2_gpu_wedged, ARMO_EV_FLUX2_GPU_WEDGED, armo_ev_flux2_gpu_wedged)
PLAIN(dds_write, ARMO_EV_DDS_WRITE, armo_ev_dds_write)
PLAIN(cuda_call, ARMO_EV_CUDA_CALL, armo_ev_cuda_call)
STRINGS(span_begin, ARMO_EV_SPAN_BEGIN, armo_ev_span, 1, const char * s0)
PLAIN(span_end, ARMO_EV_SPAN_END, armo_ev_span)

void armo_emit_rclcpp_executor_get_next_ready(void)
{
  record(ARMO_EV_RCLCPP_EXECUTOR_GET_NEXT_READY, NULL, 0, NULL, 0);
}

void armo_backend_missing(uint16_t event)
{
  armo_shm_header * h = armo_file();
  if (h != NULL) {
    __atomic_fetch_or(&h->missing[event / 64], (uint64_t)1 << (event % 64), __ATOMIC_RELAXED);
  }
}

bool armo_backend_malloc_on(void)
{
  return armo_option(ARMO_OPTION_MALLOC);
}

bool armo_backend_gpu_on(void)
{
  return armo_option(ARMO_OPTION_GPU);
}

bool armo_backend_dds_on(void)
{
  return armo_option(ARMO_OPTION_DDS);
}

// Charged to the innermost running callback, key 0 outside one (design 15.3).
static void memory(uint16_t kind, size_t bytes)
{
  if (armo_self.busy) return;
  struct armo_thread * t = armo_thread();
  if (t == NULL) return;
  const struct armo_frame * top = t->n > 0 ? &t->stack[t->n - 1] : NULL;
  if (top != NULL) {
    table_add(t, kind, top->src, top->entry, top->key, bytes);
  } else {
    table_add(t, kind, 0, 0, 0, bytes);
  }
}

void armo_backend_alloc(size_t bytes)
{
  memory(ARMO_AGG_ALLOC, bytes);
}

void armo_backend_free(size_t bytes)
{
  memory(ARMO_AGG_FREE, bytes);
}
