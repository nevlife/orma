// The cost of one ros_trace_callback_start/end pair (design 8.1): each of N threads, pinned to
// FIRST_CPU (4) onward, calls 200,000 pairs; prints the median over 5 runs of the slowest thread's
// ns per pair. Under LD_PRELOAD=liborma_preload.so, "armed" arms the file and drains the rings from
// a thread on CPU 3, as a reader does. Each run is followed by as long a pause, which keeps every
// CPU it uses at most half busy.
//
//     orma_preload_bench THREADS [armed|unarmed] [FIRST_CPU]
#define _GNU_SOURCE
#include "orma_preload/orma_shm.h"
#include "tracetools/tracetools.h"

#include <fcntl.h>
#include <glob.h>
#include <pthread.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#define PAIRS 200000
#define RUNS 5

static pthread_barrier_t start;
static volatile int done;
static orma_shm_header * header;
static int first_cpu = 4;

static uint64_t now(void)
{
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

static void pin(int cpu)
{
  cpu_set_t set;
  CPU_ZERO(&set);
  CPU_SET(cpu, &set);
  pthread_setaffinity_np(pthread_self(), sizeof set, &set);
}

static void * worker(void * arg)
{
  int i = (int)(intptr_t)arg;
  pin(first_cpu + i);
  const void * cb = (const void *)(uintptr_t)(0x1000 + i);
  ros_trace_callback_start(cb, false);
  ros_trace_callback_end(cb);
  pthread_barrier_wait(&start);
  uint64_t t0 = now();
  for (int k = 0; k < PAIRS; k++) {
    ros_trace_callback_start(cb, false);
    ros_trace_callback_end(cb);
  }
  return (void *)(uintptr_t)(now() - t0);
}

static void * drainer(void * arg)
{
  (void)arg;
  pin(3);
  while (!done) {
    for (uint32_t i = 0; i < header->n_threads; i++) {
      orma_thread_slot * s = (orma_thread_slot *)((uint8_t *)header + header->header_size +
                                                  (size_t)i * header->thread_size);
      uint64_t head = __atomic_load_n(&s->ring_head, __ATOMIC_ACQUIRE);
      __atomic_store_n(&s->ring_tail, head, __ATOMIC_RELEASE);
    }
    usleep(100);
  }
  return NULL;
}

static int cmp(const void * a, const void * b)
{
  double x = *(const double *)a, y = *(const double *)b;
  return (x > y) - (x < y);
}

int main(int argc, char ** argv)
{
  int threads = argc > 1 ? (int)strtol(argv[1], NULL, 10) : 1;
  int armed = argc > 2 && strcmp(argv[2], "armed") == 0;
  if (argc > 3) first_cpu = (int)strtol(argv[3], NULL, 10);
  pthread_t drain;
  if (armed) {
    ros_trace_rcl_init(NULL);
    char pattern[64];
    snprintf(pattern, sizeof pattern, "/dev/shm/orma.%d.*", (int)getpid());
    glob_t g;
    if (glob(pattern, 0, NULL, &g) != 0) return 1;
    int fd = open(g.gl_pathv[0], O_RDWR);
    header = mmap(NULL, lseek(fd, 0, SEEK_END), PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    __atomic_store_n(&header->armed, 1, __ATOMIC_RELAXED);
    pthread_create(&drain, NULL, drainer, NULL);
  }
  double runs[RUNS];
  for (int r = 0; r < RUNS; r++) {
    pthread_t t[64];
    pthread_barrier_init(&start, NULL, (unsigned)threads);
    for (int i = 0; i < threads; i++) pthread_create(&t[i], NULL, worker, (void *)(intptr_t)i);
    uint64_t slowest = 0;
    for (int i = 0; i < threads; i++) {
      void * ns;
      pthread_join(t[i], &ns);
      if ((uint64_t)(uintptr_t)ns > slowest) slowest = (uint64_t)(uintptr_t)ns;
    }
    pthread_barrier_destroy(&start);
    runs[r] = (double)slowest / PAIRS;
    usleep((useconds_t)(slowest / 1000));
  }
  if (armed) {
    done = 1;
    pthread_join(drain, NULL);
  }
  qsort(runs, RUNS, sizeof runs[0], cmp);
  printf(
    "threads %d %s: %.1f ns per pair (min %.1f, max %.1f)\n", threads, armed ? "armed" : "unarmed",
    runs[RUNS / 2], runs[0], runs[RUNS - 1]);
  return 0;
}
