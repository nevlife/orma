// The backend of liborma_preload.so: the shared memory file of design 13 (slots.c), made on the
// process's first event, with each thread's aggregation table and ring.
#define _GNU_SOURCE
#include "orma_preload/orma_emit.h"
#include "orma_preload/slots.h"

#include <string.h>
#include <time.h>

// Fixed in this backend (only the slot count comes from ORMA_THREADS), so the compiler turns the
// modulo of a table probe and a ring position into masks.
#define TABLE_ENTRIES ORMA_TABLE_ENTRIES_DEFAULT
#define RING_BYTES ORMA_RING_BYTES_DEFAULT
#define LOAD ORMA_LOAD
#define STORE ORMA_STORE
#define BUMP ORMA_BUMP

const struct orma_layout orma_layout = {
  .backend = ORMA_BACKEND_SHM,
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
  return b < ORMA_BUCKETS ? b : ORMA_BUCKETS - 1;
}

static void table_add(
  struct orma_thread * t, uint16_t kind, uint16_t src, uint32_t entry, uint64_t key, uint64_t value)
{
  const uint32_t n = TABLE_ENTRIES;
  uint64_t mix = (key ^ (key >> 17) ^ ((uint64_t)entry << 7) ^ kind ^ ((uint64_t)src << 3)) *
                 0x9E3779B97F4A7C15u;
  uint32_t i = (uint32_t)((mix >> 32) % n);
  for (uint32_t probe = 0; probe < n; probe++, i = i + 1 == n ? 0 : i + 1) {
    orma_table_entry * e = (orma_table_entry *)(t->slot + 1) + i;
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
  return (v + ORMA_RECORD_ALIGN - 1) & ~(ORMA_RECORD_ALIGN - 1);
}

static void ring_put(
  struct orma_thread * t, uint16_t event, uint64_t ts, const void * payload, uint32_t size,
  const char * const * strs, uint32_t n_strs)
{
  uint32_t lens[3];
  uint32_t flags = 0;
  uint32_t len = (uint32_t)sizeof(orma_record) + size;
  for (uint32_t i = 0; i < n_strs; i++) {
    size_t l = strnlen(strs[i] != NULL ? strs[i] : "", ORMA_STRING_MAX);
    if (l == ORMA_STRING_MAX) {
      l = ORMA_STRING_MAX - 1;
      flags |= ORMA_RECORD_TRUNCATED;
    }
    lens[i] = (uint32_t)l;
    len += lens[i] + 1;
  }
  len = round8(len);

  orma_thread_slot * s = t->slot;
  uint64_t head = LOAD(&s->ring_head);
  uint64_t tail = __atomic_load_n(&s->ring_tail, __ATOMIC_ACQUIRE);
  uint64_t pos = head % RING_BYTES;
  uint64_t to_end = RING_BYTES - pos;
  uint64_t need = len <= to_end ? len : to_end + len;
  if (RING_BYTES - (head - tail) < need) {
    BUMP(&s->ring_drops);
    return;
  }
  uint8_t * ring = (uint8_t *)s + orma_ring_offset(TABLE_ENTRIES);
  uint8_t * at = ring + pos;
  if (len > to_end) {
    orma_record pad = {.event = ORMA_EV_PAD, .len = (uint16_t)to_end, .t_mono_ns = ts};
    memcpy(at, &pad, to_end < sizeof pad ? to_end : sizeof pad);
    at = ring;
  }
  orma_record r = {.event = event, .len = (uint16_t)len, .flags = flags, .t_mono_ns = ts};
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
  return LOAD(&orma_header->armed) != 0;
}

// What names and links entities. It reaches the ring before the file is armed, so that a reader
// that arms it later still knows them; orma_common's catalog::init is the same list.
static bool init_event(uint16_t event)
{
  switch (event) {
    case ORMA_EV_RCL_INIT:
    case ORMA_EV_RCL_NODE_INIT:
    case ORMA_EV_RMW_PUBLISHER_INIT:
    case ORMA_EV_RCL_PUBLISHER_INIT:
    case ORMA_EV_RMW_SUBSCRIPTION_INIT:
    case ORMA_EV_RCL_SUBSCRIPTION_INIT:
    case ORMA_EV_RCLCPP_SUBSCRIPTION_INIT:
    case ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED:
    case ORMA_EV_RCL_SERVICE_INIT:
    case ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED:
    case ORMA_EV_RCL_CLIENT_INIT:
    case ORMA_EV_RCL_TIMER_INIT:
    case ORMA_EV_RCLCPP_TIMER_CALLBACK_ADDED:
    case ORMA_EV_RCLCPP_TIMER_LINK_NODE:
    case ORMA_EV_RCLCPP_CALLBACK_REGISTER:
    case ORMA_EV_RCL_TIMER_FINI:
    case ORMA_EV_RCLCPP_PUBLISHER_BASE:
    case ORMA_EV_RCLCPP_PUBLISHER_SETUP_INTRA:
    case ORMA_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA:
    case ORMA_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB:
    case ORMA_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION:
    case ORMA_EV_RCLCPP_GROUP_ADD_TIMER:
    case ORMA_EV_RCLCPP_EXECUTOR:
    case ORMA_EV_RCLCPP_COLLECTOR:
    case ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION:
    case ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP:
    case ORMA_EV_RCLCPP_EXECUTOR_ADD_GROUP:
    case ORMA_EV_RCLCPP_CALLBACK_OWNER:
    case ORMA_EV_CALLBACK_CODE:
    case ORMA_EV_FLUX_CHANNEL_INIT:
    case ORMA_EV_FLUX_ENDPOINT_INIT:
    case ORMA_EV_FLUX_GPU_WEDGED:
      return true;
    default:
      return false;
  }
}

static void record(
  uint16_t event, const void * payload, uint32_t size, const char * const * strs, uint32_t n_strs)
{
  struct orma_thread * t = orma_thread();
  if (t != NULL && (armed() || init_event(event))) {
    ring_put(t, event, now(), payload, size, strs, n_strs);
  }
}

static inline __attribute__((always_inline)) void callback_start(
  uint16_t event, const void * ev, uint32_t size, uint16_t src, uint64_t key, uint32_t entry,
  uint64_t executor)
{
  struct orma_thread * t = orma_thread();
  if (t == NULL) {
    return;
  }
  uint64_t ts = now();
  const struct orma_wake * w = &t->wake[src];
  if (w->t != 0 && w->depth == t->n + t->untracked && w->executor == executor) {
    table_add(t, ORMA_AGG_DISPATCH, src, entry, key, ts - w->t);
  }
  orma_push(t, src, key, entry, ts);
  if (armed()) {
    ring_put(t, event, ts, ev, size, NULL, 0);
  }
}

// A flux pass that delivered nothing is not counted as a callback.
static inline __attribute__((always_inline)) void callback_end(
  uint16_t event, const void * ev, uint32_t size, const char * const * strs, uint32_t n_strs,
  uint16_t src, uint64_t key, uint32_t entry, bool counted)
{
  struct orma_thread * t = orma_thread();
  if (t == NULL) {
    return;
  }
  uint64_t ts = now();
  uint64_t t0;
  if (orma_pop(t, src, key, entry, &t0) && counted) {
    table_add(t, ORMA_AGG_CALLBACK, src, entry, key, ts - t0);
  }
  if (armed()) {
    ring_put(t, event, ts, ev, size, strs, n_strs);
  }
}

static void wait_enter(uint16_t event, const void * ev, uint32_t size, uint16_t src)
{
  struct orma_thread * t = orma_thread();
  if (t == NULL) {
    return;
  }
  uint64_t ts = now();
  t->wait_start[src] = ts;
  t->wake[src].t = 0;
  if (armed()) {
    ring_put(t, event, ts, ev, size, NULL, 0);
  }
}

static void wait_exit(
  uint16_t event, const void * ev, uint32_t size, uint16_t src, uint64_t key, uint64_t executor)
{
  struct orma_thread * t = orma_thread();
  if (t == NULL) {
    return;
  }
  uint64_t ts = now();
  if (t->wait_start[src] != 0) {
    table_add(t, ORMA_AGG_WAIT, src, 0, key, ts - t->wait_start[src]);
  }
  t->wait_start[src] = 0;
  t->wake[src] = (struct orma_wake){.t = ts, .executor = executor, .depth = t->n + t->untracked};
  if (armed()) {
    ring_put(t, event, ts, ev, size, NULL, 0);
  }
}

void orma_emit_callback_start(const orma_ev_callback_start * ev)
{
  callback_start(ORMA_EV_CALLBACK_START, ev, sizeof *ev, ORMA_SRC_ROS, ev->callback, 0, 0);
}

void orma_emit_callback_end(const orma_ev_handle * ev)
{
  callback_end(ORMA_EV_CALLBACK_END, ev, sizeof *ev, NULL, 0, ORMA_SRC_ROS, ev->handle, 0, true);
}

void orma_emit_flux_callback_start(const orma_ev_flux_callback_start * ev)
{
  callback_start(
    ORMA_EV_FLUX_CALLBACK_START, ev, sizeof *ev, ORMA_SRC_FLUX, ev->executor, ev->entry,
    ev->executor);
}

void orma_emit_flux_callback_end(const orma_ev_flux_callback_end * ev, const char * segment)
{
  const char * strs[] = {segment};
  callback_end(
    ORMA_EV_FLUX_CALLBACK_END, ev, sizeof *ev, strs, 1, ORMA_SRC_FLUX, ev->executor, ev->entry,
    ev->delivered > 0);
}

void orma_emit_rcl_wait_enter(const orma_ev_rcl_wait_enter * ev)
{
  wait_enter(ORMA_EV_RCL_WAIT_ENTER, ev, sizeof *ev, ORMA_SRC_ROS);
}

void orma_emit_rcl_wait_exit(const orma_ev_rcl_wait_exit * ev)
{
  wait_exit(ORMA_EV_RCL_WAIT_EXIT, ev, sizeof *ev, ORMA_SRC_ROS, ev->wait_set, 0);
}

void orma_emit_flux_wait(const orma_ev_flux_wait * ev)
{
  wait_enter(ORMA_EV_FLUX_WAIT, ev, sizeof *ev, ORMA_SRC_FLUX);
}

void orma_emit_flux_wake(const orma_ev_flux_wake * ev)
{
  wait_exit(ORMA_EV_FLUX_WAKE, ev, sizeof *ev, ORMA_SRC_FLUX, ev->executor, ev->executor);
}

#define PLAIN(name, event, type)            \
  void orma_emit_##name(const type * ev)    \
  {                                         \
    record(event, ev, sizeof *ev, NULL, 0); \
  }

#define STRINGS(name, event, type, n, ...)            \
  void orma_emit_##name(const type * ev, __VA_ARGS__) \
  {                                                   \
    const char * strs[] = {STRING_ARGS_##n};          \
    record(event, ev, sizeof *ev, strs, n);           \
  }
#define STRING_ARGS_1 s0
#define STRING_ARGS_2 s0, s1
#define STRING_ARGS_3 s0, s1, s2

PLAIN(rcl_init, ORMA_EV_RCL_INIT, orma_ev_rcl_init)
STRINGS(
  rcl_node_init, ORMA_EV_RCL_NODE_INIT, orma_ev_rcl_node_init, 2, const char * s0, const char * s1)
PLAIN(rmw_publisher_init, ORMA_EV_RMW_PUBLISHER_INIT, orma_ev_rmw_endpoint_init)
STRINGS(rcl_publisher_init, ORMA_EV_RCL_PUBLISHER_INIT, orma_ev_rcl_topic_init, 1, const char * s0)
PLAIN(rclcpp_publish, ORMA_EV_RCLCPP_PUBLISH, orma_ev_publish)
PLAIN(rclcpp_intra_publish, ORMA_EV_RCLCPP_INTRA_PUBLISH, orma_ev_publish)
PLAIN(rcl_publish, ORMA_EV_RCL_PUBLISH, orma_ev_publish)
PLAIN(rmw_publish, ORMA_EV_RMW_PUBLISH, orma_ev_rmw_publish)
PLAIN(rmw_publish_rcl, ORMA_EV_RMW_PUBLISH_RCL, orma_ev_rmw_publish_rcl)
PLAIN(rmw_subscription_init, ORMA_EV_RMW_SUBSCRIPTION_INIT, orma_ev_rmw_endpoint_init)
STRINGS(
  rcl_subscription_init, ORMA_EV_RCL_SUBSCRIPTION_INIT, orma_ev_rcl_topic_init, 1, const char * s0)
PLAIN(rclcpp_subscription_init, ORMA_EV_RCLCPP_SUBSCRIPTION_INIT, orma_ev_rclcpp_subscription_init)
PLAIN(
  rclcpp_subscription_callback_added, ORMA_EV_RCLCPP_SUBSCRIPTION_CALLBACK_ADDED,
  orma_ev_callback_added)
PLAIN(rmw_take, ORMA_EV_RMW_TAKE, orma_ev_rmw_take)
PLAIN(rcl_take, ORMA_EV_RCL_TAKE, orma_ev_take)
PLAIN(rclcpp_take, ORMA_EV_RCLCPP_TAKE, orma_ev_take)
STRINGS(rcl_service_init, ORMA_EV_RCL_SERVICE_INIT, orma_ev_rcl_service_init, 1, const char * s0)
PLAIN(rclcpp_service_callback_added, ORMA_EV_RCLCPP_SERVICE_CALLBACK_ADDED, orma_ev_callback_added)
STRINGS(rcl_client_init, ORMA_EV_RCL_CLIENT_INIT, orma_ev_rcl_service_init, 1, const char * s0)
PLAIN(rcl_timer_init, ORMA_EV_RCL_TIMER_INIT, orma_ev_rcl_timer_init)
PLAIN(rclcpp_timer_callback_added, ORMA_EV_RCLCPP_TIMER_CALLBACK_ADDED, orma_ev_callback_added)
PLAIN(rclcpp_timer_link_node, ORMA_EV_RCLCPP_TIMER_LINK_NODE, orma_ev_rclcpp_timer_link_node)
STRINGS(
  rclcpp_callback_register, ORMA_EV_RCLCPP_CALLBACK_REGISTER, orma_ev_rclcpp_callback_register, 1,
  const char * s0)
PLAIN(
  rclcpp_executor_wait_for_work, ORMA_EV_RCLCPP_EXECUTOR_WAIT_FOR_WORK,
  orma_ev_rclcpp_executor_wait_for_work)
PLAIN(rclcpp_executor_execute, ORMA_EV_RCLCPP_EXECUTOR_EXECUTE, orma_ev_handle)
PLAIN(rcl_timer_fini, ORMA_EV_RCL_TIMER_FINI, orma_ev_handle)
PLAIN(rclcpp_publisher_base, ORMA_EV_RCLCPP_PUBLISHER_BASE, orma_ev_rclcpp_object)
PLAIN(rclcpp_publisher_setup_intra, ORMA_EV_RCLCPP_PUBLISHER_SETUP_INTRA, orma_ev_rclcpp_object)
PLAIN(
  rclcpp_subscription_setup_intra, ORMA_EV_RCLCPP_SUBSCRIPTION_SETUP_INTRA, orma_ev_rclcpp_object)
PLAIN(
  rclcpp_ipm_insert_sub_id_for_pub, ORMA_EV_RCLCPP_IPM_INSERT_SUB_ID_FOR_PUB,
  orma_ev_rclcpp_ipm_insert_sub_id_for_pub)
PLAIN(rclcpp_group_add_subscription, ORMA_EV_RCLCPP_GROUP_ADD_SUBSCRIPTION, orma_ev_rclcpp_object)
PLAIN(rclcpp_group_add_timer, ORMA_EV_RCLCPP_GROUP_ADD_TIMER, orma_ev_rclcpp_object)
PLAIN(rclcpp_executor, ORMA_EV_RCLCPP_EXECUTOR, orma_ev_rclcpp_object)
PLAIN(rclcpp_collector, ORMA_EV_RCLCPP_COLLECTOR, orma_ev_rclcpp_object)
PLAIN(
  rclcpp_collector_add_group_to_collection, ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP_TO_COLLECTION,
  orma_ev_rclcpp_object)
PLAIN(rclcpp_collector_add_group, ORMA_EV_RCLCPP_COLLECTOR_ADD_GROUP, orma_ev_rclcpp_object)
PLAIN(rclcpp_executor_add_group, ORMA_EV_RCLCPP_EXECUTOR_ADD_GROUP, orma_ev_rclcpp_object)
PLAIN(callback_code, ORMA_EV_CALLBACK_CODE, orma_ev_callback_code)
STRINGS(flux_channel_init, ORMA_EV_FLUX_CHANNEL_INIT, orma_ev_flux_channel_init, 1, const char * s0)
PLAIN(flux_claim, ORMA_EV_FLUX_CLAIM, orma_ev_flux_claim)
PLAIN(flux_loan, ORMA_EV_FLUX_LOAN, orma_ev_flux_loan)
PLAIN(flux_loan_end, ORMA_EV_FLUX_LOAN_END, orma_ev_flux_loan_end)
PLAIN(flux_commit, ORMA_EV_FLUX_COMMIT, orma_ev_flux_commit)
PLAIN(flux_take, ORMA_EV_FLUX_TAKE, orma_ev_flux_take)
PLAIN(flux_release, ORMA_EV_FLUX_RELEASE, orma_ev_flux_release)
PLAIN(flux_refused, ORMA_EV_FLUX_REFUSED, orma_ev_flux_refused)
STRINGS(
  flux_endpoint_init, ORMA_EV_FLUX_ENDPOINT_INIT, orma_ev_flux_endpoint_init, 3, const char * s0,
  const char * s1, const char * s2)
PLAIN(flux_py_gil, ORMA_EV_FLUX_PY_GIL, orma_ev_flux_py_gil)
PLAIN(flux_py_ready, ORMA_EV_FLUX_PY_READY, orma_ev_flux_py_ready)
PLAIN(flux_record_drop, ORMA_EV_FLUX_RECORD_DROP, orma_ev_flux_record_drop)
PLAIN(flux_gpu_wedged, ORMA_EV_FLUX_GPU_WEDGED, orma_ev_flux_gpu_wedged)
PLAIN(dds_write, ORMA_EV_DDS_WRITE, orma_ev_dds_write)
PLAIN(rcl_ipc_publish, ORMA_EV_RCL_IPC_PUBLISH, orma_ev_rcl_ipc_publish)
PLAIN(rcl_ipc_take, ORMA_EV_RCL_IPC_TAKE, orma_ev_rcl_ipc_take)
PLAIN(rcl_ipc_release, ORMA_EV_RCL_IPC_RELEASE, orma_ev_handle)
PLAIN(rcl_loan_borrow, ORMA_EV_RCL_LOAN_BORROW, orma_ev_rcl_loan)
PLAIN(rcl_loan_publish, ORMA_EV_RCL_LOAN_PUBLISH, orma_ev_rcl_loan)
PLAIN(rcl_loan_take, ORMA_EV_RCL_LOAN_TAKE, orma_ev_rcl_loan)
PLAIN(rmw_loan_return, ORMA_EV_RMW_LOAN_RETURN, orma_ev_rcl_loan)
PLAIN(zenoh_shm_payload, ORMA_EV_ZENOH_SHM_PAYLOAD, orma_ev_handle)
PLAIN(rclpy_take_converted, ORMA_EV_RCLPY_TAKE_CONVERTED, orma_ev_rclpy_take)
PLAIN(rclpy_publish_enter, ORMA_EV_RCLPY_PUBLISH_ENTER, orma_ev_handle)
PLAIN(cuda_call, ORMA_EV_CUDA_CALL, orma_ev_cuda_call)
STRINGS(span_begin, ORMA_EV_SPAN_BEGIN, orma_ev_span, 1, const char * s0)
PLAIN(span_end, ORMA_EV_SPAN_END, orma_ev_span)

void orma_emit_rclcpp_executor_get_next_ready(void)
{
  record(ORMA_EV_RCLCPP_EXECUTOR_GET_NEXT_READY, NULL, 0, NULL, 0);
}

void orma_backend_missing(uint16_t event)
{
  orma_shm_header * h = orma_file();
  if (h != NULL) {
    __atomic_fetch_or(&h->missing[event / 64], (uint64_t)1 << (event % 64), __ATOMIC_RELAXED);
  }
}

bool orma_backend_malloc_on(void)
{
  return orma_option(ORMA_OPTION_MALLOC);
}

bool orma_backend_gpu_on(void)
{
  return orma_option(ORMA_OPTION_GPU);
}

bool orma_backend_dds_on(void)
{
  return orma_option(ORMA_OPTION_DDS);
}

// Charged to the innermost running callback, key 0 outside one (design 15.3).
static void memory(uint16_t kind, size_t bytes)
{
  if (orma_self.busy) {
    return;
  }
  struct orma_thread * t = orma_thread();
  if (t == NULL) {
    return;
  }
  const struct orma_frame * top = t->n > 0 ? &t->stack[t->n - 1] : NULL;
  if (top != NULL) {
    table_add(t, kind, top->src, top->entry, top->key, bytes);
  } else {
    table_add(t, kind, 0, 0, 0, bytes);
  }
}

void orma_backend_alloc(size_t bytes)
{
  memory(ORMA_AGG_ALLOC, bytes);
}

void orma_backend_free(size_t bytes)
{
  memory(ORMA_AGG_FREE, bytes);
}
