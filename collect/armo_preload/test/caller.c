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
#define _GNU_SOURCE
#include "armo_preload/armo_events.h"
#include "armo_preload/armo_shm.h"
#include "armo_preload/armo_tracepoints.h"
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

int main(int argc, char ** argv)
{
  ros_trace_rcl_init(NULL);
  ros_trace_rcl_publish(K(0x1), NULL);
  open_own_file();
  // Before arming, init events reach the ring and the others do not.
  const armo_thread_slot * own = slot_of((uint32_t)gettid());
  CHECK(records(own, ARMO_EV_RCL_INIT, 0) == 1 && records(own, ARMO_EV_RCL_PUBLISH, 0) == 0);
  __atomic_store_n(&header->armed, 1, __ATOMIC_RELAXED);
  if (argc == 3) {
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
