// Calls the caught functions a known number of times under LD_PRELOAD=libarmo_preload.so and
// checks its own shared memory file: exact table counts across threads, nested and recursive
// callbacks, dispatch after rcl_wait, gated forms, callback code words, stamps, string cuts, depth,
// table, ring and slot overflow, slot reuse, and fork. Run with ARMO_THREADS=8.
//
//     caller                       the ROS checks
//     caller asan                  the same but malloc attribution, with libasan.so preloaded first
//     caller flux2-global STUB     flux2 events of a stub loaded RTLD_GLOBAL
//     caller flux2-local STUB      the same loaded RTLD_LOCAL, as Python loads libflux2_ffi
//     caller flux2-none STUB       a stub without flux2_trace_contract_2: no events
//     caller collect CUDA DDS      spans, and stub CUDA and DDS calls with their options off and on
#define _GNU_SOURCE
#include "armo_preload/armo_events.h"
#include "armo_preload/armo_shm.h"
#include "armo_preload/armo_tracepoints.h"
#include "armo_preload/cuda_api.h"
#include "armo_span/span.h"
#include "tracetools/tracetools.h"

#include <dlfcn.h>
#include <fcntl.h>
#include <glob.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

typedef struct rcl_wait_set_s rcl_wait_set_t;
int rcl_wait(rcl_wait_set_t * wait_set, int64_t timeout);

#define CHECK(c)                                                            \
  do {                                                                      \
    if (!(c)) {                                                             \
      fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__, __LINE__, #c); \
      exit(1);                                                              \
    }                                                                       \
  } while (0)

#define ROUNDS UINT64_C(500)
#define WORKERS 4

static armo_shm_header * header;
static int fd;
// ASan's allocator, preloaded first, serves the process: the shim sees no malloc.
static int asan;

static armo_thread_slot * slot_at(uint32_t i)
{
  return (armo_thread_slot *)((uint8_t *)header + header->header_size +
                              (size_t)i * header->thread_size);
}

static armo_thread_slot * slot_of(uint32_t tid)
{
  for (uint32_t i = 0; i < header->n_threads; i++) {
    armo_thread_slot * s = slot_at(i);
    if (s->tid == tid && s->state != ARMO_THREAD_FREE) return s;
  }
  return NULL;
}

static const armo_table_entry * row(
  const armo_thread_slot * s, uint16_t kind, uint16_t src, uint32_t entry, uint64_t key)
{
  const armo_table_entry * t = (const armo_table_entry *)(s + 1);
  for (uint32_t i = 0; i < header->table_entries; i++) {
    if (t[i].kind == kind && t[i].src == src && t[i].entry == entry && t[i].key == key)
      return &t[i];
  }
  return NULL;
}

static uint64_t count(const armo_thread_slot * s, uint16_t kind, uint64_t key)
{
  const armo_table_entry * e = row(s, kind, ARMO_SRC_ROS, 0, key);
  if (e == NULL) return 0;
  uint64_t hist = 0;
  for (int b = 0; b < ARMO_BUCKETS; b++) hist += e->hist[b];
  CHECK(hist == e->count && e->max <= e->sum);
  return e->count;
}

// Calls fn on each record in the slot's ring between tail and head.
static void each_record(
  const armo_thread_slot * s, void (*fn)(const armo_record *, void *), void * arg)
{
  const uint8_t * ring = (const uint8_t *)s + armo_ring_offset(header->table_entries);
  uint64_t head = __atomic_load_n(&s->ring_head, __ATOMIC_ACQUIRE);
  for (uint64_t at = s->ring_tail; at < head;) {
    const armo_record * r = (const armo_record *)(ring + at % header->ring_bytes);
    CHECK(r->len % 8 == 0 && r->len >= 8);
    if (r->event != ARMO_EV_PAD) fn(r, arg);
    at += r->len;
  }
}

struct tally
{
  uint64_t key;
  uint16_t event;
  uint64_t n;
};

static void tally_one(const armo_record * r, void * arg)
{
  struct tally * t = arg;
  uint64_t first;
  memcpy(&first, r + 1, sizeof first);
  if (r->event == t->event && (t->key == 0 || first == t->key)) t->n++;
}

static uint64_t records(const armo_thread_slot * s, uint16_t event, uint64_t key)
{
  struct tally t = {.key = key, .event = event};
  each_record(s, tally_one, &t);
  return t.n;
}

static const armo_record * last;
static void keep_last(const armo_record * r, void * arg)
{
  if (r->event == *(uint16_t *)arg) last = r;
}

static const armo_record * last_record(const armo_thread_slot * s, uint16_t event)
{
  last = NULL;
  each_record(s, keep_last, &event);
  CHECK(last != NULL);
  return last;
}

static void open_own_file(void)
{
  char pattern[64];
  snprintf(pattern, sizeof pattern, "/dev/shm/armo.%d.*", (int)getpid());
  glob_t g;
  CHECK(glob(pattern, 0, NULL, &g) == 0 && g.gl_pathc == 1);
  fd = open(g.gl_pathv[0], O_RDWR | O_CLOEXEC);
  CHECK(fd >= 0);
  globfree(&g);
  off_t size = lseek(fd, 0, SEEK_END);
  header = mmap(NULL, (size_t)size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  CHECK(
    header != MAP_FAILED && header->magic == ARMO_SHM_MAGIC && header->pid == (uint32_t)getpid());
  CHECK(header->n_threads == 8 && header->backend == ARMO_BACKEND_SHM);
  CHECK((size_t)size == header->header_size + (size_t)header->n_threads * header->thread_size);
}

static void reader_lock(short type)
{
  struct flock fl = {.l_type = type, .l_whence = SEEK_SET, .l_start = ARMO_LOCK_READER, .l_len = 1};
  CHECK(fcntl(fd, F_OFD_SETLK, &fl) == 0);
}

#define K(n) ((const void *)(uintptr_t)(n))

static uint32_t worker_tids[WORKERS];

static void * worker(void * arg)
{
  int i = (int)(intptr_t)arg;
  worker_tids[i] = (uint32_t)gettid();
  const void * a = K(0x1000 + i);
  const void * b = K(0x2000 + i);
  const void * c = K(0x3000 + i);
  for (uint64_t r = 0; r < ROUNDS; r++) {
    ros_trace_callback_start(a, false);
    ros_trace_callback_start(b, false);
    ros_trace_callback_end(b);
    ros_trace_callback_start(b, true);
    ros_trace_callback_end(b);
    ros_trace_callback_end(a);
    ros_trace_callback_start(c, false);
    ros_trace_callback_start(c, false);
    ros_trace_callback_end(c);
    ros_trace_callback_end(c);
  }
  return NULL;
}

static void check_workers(void)
{
  pthread_t t[WORKERS];
  for (int i = 0; i < WORKERS; i++) pthread_create(&t[i], NULL, worker, (void *)(intptr_t)i);
  for (int i = 0; i < WORKERS; i++) pthread_join(t[i], NULL);
  for (int i = 0; i < WORKERS; i++) {
    armo_thread_slot * s = slot_of(worker_tids[i]);
    CHECK(s != NULL && s->state == ARMO_THREAD_EXITED && s->depth == 0 && s->cur_cb == 0);
    CHECK(count(s, ARMO_AGG_CALLBACK, 0x1000 + i) == ROUNDS);
    CHECK(count(s, ARMO_AGG_CALLBACK, 0x2000 + i) == 2 * ROUNDS);
    CHECK(count(s, ARMO_AGG_CALLBACK, 0x3000 + i) == 2 * ROUNDS);
    CHECK(count(s, ARMO_AGG_CALLBACK, 0x1000 + (i + 1) % WORKERS) == 0);
    CHECK(records(s, ARMO_EV_CALLBACK_START, 0x2000 + i) == 2 * ROUNDS);
    CHECK(records(s, ARMO_EV_CALLBACK_END, 0) == 5 * ROUNDS);
    CHECK(s->ring_drops == 0 && s->table_overflow == 0 && s->depth_overflow == 0);
  }
}

// The main thread: rcl_wait goes on to librcl (which refuses the NULL wait set) and the next
// callback measures dispatch from its return; the other functions with their fields.
static void check_fields(void)
{
  armo_thread_slot * s = slot_of((uint32_t)gettid());
  CHECK(s != NULL && s->state == ARMO_THREAD_LIVE);
  CHECK(rcl_wait(NULL, 5) == 11);
  ros_trace_callback_start(K(0xD), false);
  CHECK(s->depth == 1 && s->cur_cb == 0xD && s->cur_src == ARMO_SRC_ROS);
  ros_trace_callback_end(K(0xD));
  CHECK(count(s, ARMO_AGG_DISPATCH, 0xD) == 1 && count(s, ARMO_AGG_CALLBACK, 0xD) == 1);
  CHECK(count(s, ARMO_AGG_WAIT, 0) == 1);
  const armo_ev_rcl_wait_exit * exit_ev = (const void *)(last_record(s, ARMO_EV_RCL_WAIT_EXIT) + 1);
  CHECK(exit_ev->ret == 11);

#if ARMO_TP_GATES
  CHECK(ros_trace_enabled_rclcpp_callback_register());
  ros_trace_do_rclcpp_callback_register(K(0xE), "void on_message(int)");
#else
  ros_trace_rclcpp_callback_register(K(0xE), "void on_message(int)");
#endif
  const armo_record * r = last_record(s, ARMO_EV_RCLCPP_CALLBACK_REGISTER);
  CHECK(
    strcmp(
      (const char *)(r + 1) + sizeof(armo_ev_rclcpp_callback_register), "void on_message(int)") ==
    0);

  uint64_t words[6] = {0, 8, 16, 24, 32, 40};
  ros_trace_rclcpp_subscription_callback_added(K(0x5), words);
  const armo_ev_callback_code * code = (const void *)(last_record(s, ARMO_EV_CALLBACK_CODE) + 1);
  CHECK(code->callback == (uintptr_t)words);
  CHECK(code->words[0] == 24 && code->words[1] == 16 && code->words[2] == 32);
  CHECK(code->words[3] == 40 && code->words[4] == 8);

  // A handle 16 bytes before an unreadable page: the words past it read as 0, without a fault.
  long page = sysconf(_SC_PAGESIZE);
  uint8_t * two = mmap(NULL, 2 * page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  CHECK(two != MAP_FAILED && mprotect(two + page, page, PROT_NONE) == 0);
  uint64_t * edge = (uint64_t *)(two + page - 16);
  edge[1] = 77;
  ros_trace_rclcpp_timer_callback_added(K(0x6), edge);
  code = (const void *)(last_record(s, ARMO_EV_CALLBACK_CODE) + 1);
  CHECK(code->words[4] == 77 && code->words[0] == 0 && code->words[3] == 0);

  uint64_t message[2] = {0x1122334455667788u, 0};
  ros_trace_rcl_publish(K(0x7), message);
  const armo_ev_publish * pub = (const void *)(last_record(s, ARMO_EV_RCL_PUBLISH) + 1);
  CHECK(pub->publisher == 0x7 && pub->stamp == message[0]);
  ros_trace_rmw_take(K(0x8), message, 42, false);
  const armo_ev_rmw_take * take = (const void *)(last_record(s, ARMO_EV_RMW_TAKE) + 1);
  CHECK(take->stamp == 0 && take->source_timestamp == 42 && take->taken == 0);

  char name[300];
  memset(name, 'n', sizeof name - 1);
  name[sizeof name - 1] = '\0';
  ros_trace_rcl_node_init(K(0x9), K(0xA), name, "/ns");
  r = last_record(s, ARMO_EV_RCL_NODE_INIT);
  const char * strs = (const char *)(r + 1) + sizeof(armo_ev_rcl_node_init);
  CHECK(r->flags == ARMO_RECORD_TRUNCATED && strlen(strs) == ARMO_STRING_MAX - 1);
  CHECK(strcmp(strs + ARMO_STRING_MAX, "/ns") == 0 && r->len % 8 == 0);

  if (asan) return;
  // malloc attribution, switched on by the reader: inside a callback to its key, outside to 0.
  __atomic_store_n(&header->options, ARMO_OPTION_MALLOC, __ATOMIC_RELAXED);
  ros_trace_callback_start(K(0xF), false);
  void * volatile p = malloc(1000);
  free(p);
  ros_trace_callback_end(K(0xF));
  p = malloc(3000);
  __atomic_store_n(&header->options, 0, __ATOMIC_RELAXED);
  free(p);
  const armo_table_entry * in = row(s, ARMO_AGG_ALLOC, ARMO_SRC_ROS, 0, 0xF);
  const armo_table_entry * out = row(s, ARMO_AGG_ALLOC, 0, 0, 0);
  CHECK(in != NULL && in->count == 1 && in->sum >= 1000 && in->sum < 1100);
  CHECK(row(s, ARMO_AGG_FREE, ARMO_SRC_ROS, 0, 0xF)->sum == in->sum);
  CHECK(
    out != NULL && out->count == 1 && out->sum >= 3000 && row(s, ARMO_AGG_FREE, 0, 0, 0) == NULL);
}

static void * nest_deep(void * arg)
{
  (void)arg;
  for (int k = 0; k < 20; k++) ros_trace_callback_start(K(0x100 + k), false);
  armo_thread_slot * s = slot_of((uint32_t)gettid());
  CHECK(s->depth == 20 && s->depth_overflow == 4 && s->cur_cb == 0x100 + 15);
  for (int k = 19; k >= 0; k--) ros_trace_callback_end(K(0x100 + k));
  CHECK(s->depth == 0 && s->cur_cb == 0);
  for (int k = 0; k < 16; k++) CHECK(count(s, ARMO_AGG_CALLBACK, 0x100 + k) == 1);
  for (int k = 16; k < 20; k++) CHECK(count(s, ARMO_AGG_CALLBACK, 0x100 + k) == 0);
  return NULL;
}

// A thread's slot is faulted in when the thread takes it, ring included, before the ring fills:
// the few records of a fresh slot touched only its first ring page.
static void check_resident(const armo_thread_slot * s)
{
  uintptr_t page = (uintptr_t)sysconf(_SC_PAGESIZE);
  uintptr_t from = (uintptr_t)s & ~(page - 1);
  size_t pages = ((uintptr_t)s + header->thread_size - from + page - 1) / page;
  unsigned char * resident = calloc(pages, 1);
  CHECK(mincore((void *)from, pages * page, resident) == 0);
  for (size_t i = 0; i < pages; i++) CHECK(resident[i] & 1);
  free(resident);
}

static void * many_keys(void * arg)
{
  (void)arg;
  for (int k = 0; k < 130; k++) {
    ros_trace_callback_start(K(0x200 + k), false);
    ros_trace_callback_end(K(0x200 + k));
  }
  armo_thread_slot * s = slot_of((uint32_t)gettid());
  uint64_t rows = 0;
  for (int k = 0; k < 130; k++) rows += count(s, ARMO_AGG_CALLBACK, 0x200 + k);
  CHECK(rows == 128 && s->table_overflow == 2);
  check_resident(s);
  return NULL;
}

#define PAIRS 9000

static void * flood(void * arg)
{
  (void)arg;
  for (int k = 0; k < PAIRS; k++) {
    ros_trace_callback_start(K(0x300), false);
    ros_trace_callback_end(K(0x300));
  }
  armo_thread_slot * s = slot_of((uint32_t)gettid());
  // A start record is 32 bytes and an end record 24: whole pairs fill the ring to 8 bytes short.
  uint64_t fit = header->ring_bytes / 56;
  CHECK(s->ring_head - s->ring_tail == 56 * fit);
  CHECK(records(s, ARMO_EV_CALLBACK_START, 0) == fit && records(s, ARMO_EV_CALLBACK_END, 0) == fit);
  CHECK(s->ring_drops == 2 * (PAIRS - fit));
  CHECK(count(s, ARMO_AGG_CALLBACK, 0x300) == PAIRS);
  return NULL;
}

static void run(void * (*fn)(void *))
{
  pthread_t t;
  pthread_create(&t, NULL, fn, NULL);
  pthread_join(t, NULL);
}

static pthread_barrier_t all_live;

static void * touch(void * arg)
{
  (void)arg;
  ros_trace_rcl_take(NULL);
  return NULL;
}

static void * hold(void * arg)
{
  touch(arg);
  pthread_barrier_wait(&all_live);
  pthread_barrier_wait(&all_live);
  return NULL;
}

// The main thread keeps one of the 8 slots. Without a reader, threads reuse exited slots; 8
// threads alive together leave one uncounted; with a reader present exited slots wait for it.
static void check_slots(void)
{
  for (int i = 0; i < 20; i++) run(touch);
  CHECK(header->threads_overflow == 0);

  pthread_t t[8];
  pthread_barrier_init(&all_live, NULL, 9);
  for (int i = 0; i < 8; i++) pthread_create(&t[i], NULL, hold, NULL);
  pthread_barrier_wait(&all_live);
  CHECK(header->threads_overflow == 1);
  pthread_barrier_wait(&all_live);
  for (int i = 0; i < 8; i++) pthread_join(t[i], NULL);

  reader_lock(F_WRLCK);
  run(touch);
  CHECK(header->threads_overflow == 2);
  reader_lock(F_UNLCK);
  run(touch);
  CHECK(header->threads_overflow == 2);
}

// The child writes a file of its own and removes only that one at exit.
static void check_fork(void)
{
  pid_t child = fork();
  if (child == 0) {
    ros_trace_callback_start(K(0x400), false);
    ros_trace_callback_end(K(0x400));
    char pattern[64];
    snprintf(pattern, sizeof pattern, "/dev/shm/armo.%d.*", (int)getpid());
    glob_t g;
    exit(glob(pattern, 0, NULL, &g) == 0 && g.gl_pathc == 1 ? 0 : 1);
  }
  int status = 0;
  CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
  char path[64];
  snprintf(
    path, sizeof path, "/dev/shm/armo.%d.%llu", (int)getpid(),
    (unsigned long long)header->starttime);
  CHECK(access(path, F_OK) == 0);
  snprintf(path, sizeof path, "/dev/shm/armo.%d.*", (int)child);
  glob_t g;
  CHECK(glob(path, 0, NULL, &g) == GLOB_NOMATCH);
}

static uint64_t flux2_count(uint16_t kind, uint32_t entry, uint64_t key)
{
  const armo_table_entry * e = row(slot_of((uint32_t)gettid()), kind, ARMO_SRC_FLUX2, entry, key);
  return e != NULL ? e->count : 0;
}

struct flux2_tally
{
  uint64_t events;
  uint64_t commits;
  uint64_t last_stamp;
  int strings_ok;
  int32_t wedged;
};

static void flux2_one(const armo_record * r, void * arg)
{
  struct flux2_tally * t = arg;
  if (r->event < ARMO_EV_FLUX2_CHANNEL_INIT || r->event > ARMO_EV_FLUX2_GPU_WEDGED) return;
  t->events++;
  if (r->event == ARMO_EV_FLUX2_GPU_WEDGED)
    t->wedged = ((const armo_ev_flux2_gpu_wedged *)(r + 1))->result;
  if (r->event == ARMO_EV_FLUX2_COMMIT) {
    t->commits++;
    t->last_stamp = ((const armo_ev_flux2_commit *)(r + 1))->stamp;
  }
  if (r->event == ARMO_EV_FLUX2_ENDPOINT_INIT) {
    const char * s = (const char *)(r + 1) + sizeof(armo_ev_flux2_endpoint_init);
    t->strings_ok = strcmp(s, "/armo_test/node") == 0 && strcmp(s + 16, "/key") == 0 &&
                    strcmp(s + 21, "/flux2.v") == 0;
  }
}

#define FLUX2_ROUNDS 200

static void check_flux2(const char * mode, const char * stub)
{
  void * lib =
    dlopen(stub, RTLD_NOW | (strcmp(mode, "flux2-global") == 0 ? RTLD_GLOBAL : RTLD_LOCAL));
  CHECK(lib != NULL);
  void (*stub_run)(int) = (void (*)(int))dlsym(lib, "stub_run");
  uint64_t (*stub_calls)(void) = (uint64_t(*)(void))dlsym(lib, "stub_calls");
  void (*stub_wedge)(int32_t) = (void (*)(int32_t))dlsym(lib, "stub_wedge");
  // A wedge is once per process and reaches the ring before arming, as init events do.
  __atomic_store_n(&header->armed, 0, __ATOMIC_RELAXED);
  stub_wedge(700);
  __atomic_store_n(&header->armed, 1, __ATOMIC_RELAXED);
  stub_run(FLUX2_ROUNDS);
  CHECK(stub_calls() == 11 + 10 * FLUX2_ROUNDS);

  struct flux2_tally t = {0};
  each_record(slot_of((uint32_t)gettid()), flux2_one, &t);
  if (strcmp(mode, "flux2-none") == 0) {
    CHECK(t.events == 0 && flux2_count(ARMO_AGG_CALLBACK, 1, 0xE0) == 0);
    return;
  }
  CHECK(t.events == 11 + 10 * FLUX2_ROUNDS && t.commits == FLUX2_ROUNDS && t.strings_ok);
  CHECK(t.wedged == 700);
  CHECK(t.last_stamp == 0x5000000000u + FLUX2_ROUNDS);
  CHECK(flux2_count(ARMO_AGG_CALLBACK, 1, 0xE0) == FLUX2_ROUNDS);
  CHECK(flux2_count(ARMO_AGG_CALLBACK, 3, 0xE2) == FLUX2_ROUNDS);
  CHECK(flux2_count(ARMO_AGG_DISPATCH, 1, 0xE0) == FLUX2_ROUNDS);
  CHECK(flux2_count(ARMO_AGG_DISPATCH, 3, 0xE2) == 0);
  CHECK(flux2_count(ARMO_AGG_WAIT, 0, 0xE0) == FLUX2_ROUNDS);
}

// Whether `f` is a function this process's libarmo_preload.so defines, named `name` if given.
static int ours(void * f, const char * name)
{
  Dl_info at;
  return f != NULL && dladdr(f, &at) != 0 && strstr(at.dli_fname, "libarmo_preload") != NULL &&
         (name == NULL || (at.dli_sname != NULL && strcmp(at.dli_sname, name) == 0));
}

static uint64_t count_records(uint16_t event)
{
  return records(slot_of((uint32_t)gettid()), event, 0);
}

static const char * span_name(const armo_record * r)
{
  return (const char *)(r + 1) + sizeof(armo_ev_span);
}

struct cuda_tally
{
  uint64_t n, by_api[ARMO_CUDA_API_COUNT], bytes, failed, slow;
};

static void cuda_one(const armo_record * r, void * arg)
{
  struct cuda_tally * t = arg;
  if (r->event != ARMO_EV_CUDA_CALL) return;
  const armo_ev_cuda_call * c = (const void *)(r + 1);
  t->n++;
  t->by_api[c->api]++;
  t->bytes += c->bytes;
  t->failed += c->result != 0;
  t->slow += c->duration_ns >= 2000000;
}

#define CUDA_WRAPPERS UINT64_C(53)
// Every entry is called once through each lookup.
#define PASSES UINT64_C(2)

static const armo_cuda_memcpy2d plane = {.width = 4, .height = 8};
static const armo_cuda_memcpy3d box = {.width = 2, .height = 3, .depth = 5};

// Bytes of one pass over every entry: 64 for each of 26 plain copies and allocs, 4 x 8 for each
// of 6 2D copies and 2 x 3 x 5 for each of 4 3D ones.
#define CUDA_PASS_BYTES UINT64_C(1976)

// Each entry through `lookup`, which must hand out armo's function of that name, and each call on
// to the stub's. The locals are the entries' parameter names.
static void cuda_pass(void * (*lookup)(const char *), const char * (*stub_last)(void))
{
  CUdeviceptr ddst = 0, dsrc = 0, dptr = 0, out = 0, *dptr_out = &out;
  char buf[64];
  void *hdst = buf, *hptr = buf, *host_out = NULL, **hptr_out = &host_out;
  const void * hsrc = buf;
  size_t count = sizeof buf;
  CUcontext ctx = NULL, dctx = NULL, sctx = NULL;
  CUstream stream = NULL;
  CUevent event = NULL;
  CUfunction f = NULL;
  CUgraphExec graph = NULL;
  const CUlaunchConfig * config = NULL;
  unsigned int gx = 1, gy = 1, gz = 1, bx = 1, by = 1, bz = 1, shared = 0, flags = 0;
  void ** kparams = NULL;
  void ** extra = NULL;
  const armo_cuda_memcpy2d * copy2d = &plane;
  const armo_cuda_memcpy3d * copy3d = &box;
#define CALL(name, api, bytes, params, args) \
  {                                          \
    void * p = lookup(#name);                \
    CHECK(ours(p, #name));                   \
    ((CUresult(*) params)p) args;            \
    CHECK(strcmp(stub_last(), #name) == 0);  \
  }
  ARMO_CUDA_FUNCTIONS(CALL)
#undef CALL
}

static void * global_lookup(const char * name)
{
  return dlsym(RTLD_DEFAULT, name);
}

static armo_cu_get_proc_address_v2 gpa;

// As cudart asks: the name without _v2, _ptds or _ptsz, and the per-thread flag for those two.
static void * driver_lookup(const char * name)
{
  char base[96];
  snprintf(base, sizeof base, "%s", name);
  uint64_t flags = 0;
  char * cut = strstr(base, "_pt");
  if (cut != NULL) {
    *cut = '\0';
    flags = ARMO_CU_PER_THREAD;
  }
  cut = strstr(base, "_v2");
  int version = cut != NULL || strcmp(base, "cuCtxSynchronize") != 0 ? 13000 : 2000;
  if (cut != NULL) *cut = '\0';
  void * p = NULL;
  int status = -1;
  CHECK(gpa(base, &p, version, flags, &status) == 0 && status == 0);
  return p;
}

static void check_cuda(const char * stub)
{
  void * lib = dlopen(stub, RTLD_NOW | RTLD_LOCAL);
  CHECK(lib != NULL);
  const char * (*stub_last)(void) = (const char * (*)(void))dlsym(lib, "stub_last");
  unsigned long (*stub_calls)(void) = (unsigned long (*)(void))dlsym(lib, "stub_calls");

  // cudart's way in: cuGetProcAddress from its own libcuda handle, then itself at 12000 as _v2.
  armo_cu_get_proc_address gpa_v1 = (armo_cu_get_proc_address)dlsym(lib, "cuGetProcAddress");
  CHECK(ours((void *)gpa_v1, "cuGetProcAddress"));
  void * p = NULL;
  CHECK(gpa_v1("cuGetProcAddress", &p, 12000, 0) == 0 && ours(p, "cuGetProcAddress_v2"));
  gpa = (armo_cu_get_proc_address_v2)p;
  CHECK(gpa_v1("cuGetProcAddress", &p, 11030, 0) == 0 && ours(p, "cuGetProcAddress"));
  CHECK(dlsym(lib, "cuGetProcAddress_v2") == (void *)gpa);
  CHECK(!ours(dlsym(lib, "stub_last"), NULL));

  // Off: straight to the stub, no record.
  CUdeviceptr d;
  CUresult (*alloc)(CUdeviceptr *, size_t) =
    (CUresult(*)(CUdeviceptr *, size_t))dlsym(lib, "cuMemAlloc_v2");
  CHECK(ours((void *)alloc, "cuMemAlloc_v2") && alloc(&d, 64) == 2);
  CHECK(strcmp(stub_last(), "cuMemAlloc_v2") == 0 && count_records(ARMO_EV_CUDA_CALL) == 0);

  __atomic_store_n(&header->options, ARMO_OPTION_GPU, __ATOMIC_RELAXED);
  unsigned long before = stub_calls();
  cuda_pass(global_lookup, stub_last);
  cuda_pass(driver_lookup, stub_last);
  CHECK(stub_calls() - before == PASSES * CUDA_WRAPPERS);
  __atomic_store_n(&header->options, 0, __ATOMIC_RELAXED);
  CHECK(alloc(&d, 64) == 2);

  struct cuda_tally t = {0};
  each_record(slot_of((uint32_t)gettid()), cuda_one, &t);
  CHECK(t.n == PASSES * CUDA_WRAPPERS && t.bytes == PASSES * CUDA_PASS_BYTES);
  CHECK(t.by_api[ARMO_CUDA_LAUNCH] == PASSES * 6 && t.by_api[ARMO_CUDA_GRAPH_LAUNCH] == PASSES * 2);
  CHECK(
    t.by_api[ARMO_CUDA_MEMCPY] == PASSES * 16 && t.by_api[ARMO_CUDA_MEMCPY_ASYNC] == PASSES * 14);
  CHECK(
    t.by_api[ARMO_CUDA_SYNC_STREAM] == PASSES * 2 && t.by_api[ARMO_CUDA_SYNC_CTX] == PASSES * 2);
  CHECK(t.by_api[ARMO_CUDA_SYNC_EVENT] == PASSES && t.by_api[ARMO_CUDA_ALLOC] == PASSES * 6);
  CHECK(t.by_api[ARMO_CUDA_FREE] == PASSES * 4);
  // cuMemAlloc_v2's error comes through; the stub's cuStreamSynchronize sleeps 2 ms.
  CHECK(t.failed == PASSES && t.slow >= PASSES);
}

typedef uintptr_t (*write1)(uintptr_t, uintptr_t);
typedef uintptr_t (*write2)(uintptr_t, uintptr_t, uintptr_t);

static void check_dds(const char * stub)
{
  // Global: librcl's constructor loaded the default rmw's DDS library, RTLD_LOCAL, and a search of
  // the loaded objects would come to it before the stub.
  void * lib = dlopen(stub, RTLD_NOW | RTLD_GLOBAL);
  CHECK(lib != NULL);
  const char * (*stub_last)(void) = (const char * (*)(void))dlsym(lib, "stub_last");
  int32_t (*cyclone[3])(int32_t, const void *);
  const char * cyclone_names[3] = {"dds_write", "dds_writecdr", "dds_forwardcdr"};
  for (int i = 0; i < 3; i++) {
    cyclone[i] = (int32_t(*)(int32_t, const void *))dlsym(RTLD_DEFAULT, cyclone_names[i]);
    CHECK(ours((void *)cyclone[i], cyclone_names[i]));
  }
  write1 fast2 = (write1)dlsym(RTLD_DEFAULT, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPv");
  write2 fast2p = (write2)dlsym(
    RTLD_DEFAULT, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPvRNS_8fastrtps4rtps11WriteParamsE");
  write1 fast3 = (write1)dlsym(RTLD_DEFAULT, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPKv");
  write2 fast3p = (write2)dlsym(
    RTLD_DEFAULT, "_ZN8eprosima7fastdds3dds10DataWriter5writeEPKvRNS0_4rtps11WriteParamsE");
  CHECK(ours((void *)fast2, NULL) && ours((void *)fast2p, NULL));
  CHECK(ours((void *)fast3, NULL) && ours((void *)fast3p, NULL));

  int32_t (*write_ts)(int32_t, const void *, int64_t) =
    (int32_t(*)(int32_t, const void *, int64_t))dlsym(RTLD_DEFAULT, "dds_write_ts");
  CHECK(ours((void *)write_ts, "dds_write_ts"));

  for (int on = 0; on < 2; on++) {
    __atomic_store_n(&header->options, on ? ARMO_OPTION_DDS : 0, __ATOMIC_RELAXED);
    for (int i = 0; i < 3; i++) {
      CHECK(cyclone[i](0x1234 + i, NULL) == 0 && strcmp(stub_last(), cyclone_names[i]) == 0);
    }
    CHECK(write_ts(0x1234, NULL, 7) == 0 && strcmp(stub_last(), "dds_write_ts") == 0);
    CHECK(fast2(0xBEEF, 0) == 1 && fast2p(0xBEEF, 0, 0) == 1);
    CHECK(fast3(0xBEEF, 0) == 0 && fast3p(0xBEEF, 0, 0) == 0);
    CHECK(strstr(stub_last(), "EPKvRNS0_4rtps11WriteParamsE") != NULL);
    CHECK(count_records(ARMO_EV_DDS_WRITE) == (on ? 8u : 0u));
  }
  __atomic_store_n(&header->options, 0, __ATOMIC_RELAXED);
  const armo_thread_slot * s = slot_of((uint32_t)gettid());
  CHECK(records(s, ARMO_EV_DDS_WRITE, 0x1234) == 2 && records(s, ARMO_EV_DDS_WRITE, 0x1235) == 1);
  CHECK(records(s, ARMO_EV_DDS_WRITE, 0xBEEF) == 4);
}

// Spans need no option; the key of an end is the address its begin named.
static void check_spans(void)
{
  static const char outer[] = "outer";
  static const char inner[] = "inner";
  for (int i = 0; i < 3; i++) {
    armo_span_begin(outer);
    armo_span_begin(inner);
    armo_span_end(inner);
    armo_span_end(outer);
  }
  const armo_thread_slot * s = slot_of((uint32_t)gettid());
  CHECK(records(s, ARMO_EV_SPAN_BEGIN, (uintptr_t)outer) == 3);
  CHECK(records(s, ARMO_EV_SPAN_BEGIN, (uintptr_t)inner) == 3);
  CHECK(records(s, ARMO_EV_SPAN_END, (uintptr_t)outer) == 3);
  CHECK(records(s, ARMO_EV_SPAN_END, (uintptr_t)inner) == 3);
  CHECK(strcmp(span_name(last_record(s, ARMO_EV_SPAN_BEGIN)), "inner") == 0);
}

static void check_collect(const char * cuda_stub, const char * dds_stub)
{
  // dlsym forwards RTLD_NEXT as a tail call, so it still means after this program.
  CHECK(ours(dlsym(RTLD_NEXT, "rcl_wait"), "rcl_wait"));
  check_spans();
  check_cuda(cuda_stub);
  check_dds(dds_stub);
}

int main(int argc, char ** argv)
{
  ros_trace_rcl_init(NULL);
  ros_trace_rcl_publish(K(0x1), NULL);
  open_own_file();
  // Before arming, init events reach the ring and the others do not.
  const armo_thread_slot * own = slot_of((uint32_t)gettid());
  CHECK(records(own, ARMO_EV_RCL_INIT, 0) == 1 && records(own, ARMO_EV_RCL_PUBLISH, 0) == 0);
  __atomic_store_n(&header->armed, 1, __ATOMIC_RELAXED);
  if (argc == 4 && strcmp(argv[1], "collect") == 0) {
    check_collect(argv[2], argv[3]);
  } else if (argc == 3) {
    check_flux2(argv[1], argv[2]);
  } else {
    asan = argc == 2 && strcmp(argv[1], "asan") == 0;
    check_workers();
    check_fields();
    run(nest_deep);
    run(many_keys);
    run(flood);
    check_slots();
    check_fork();
  }
  for (int w = 0; w < ARMO_MISSING_WORDS; w++) CHECK(header->missing[w] == 0);
  printf("ok\n");
  return 0;
}
