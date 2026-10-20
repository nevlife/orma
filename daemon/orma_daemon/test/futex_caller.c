// Futex waits of one thread, for accuracy.py's futex case (design 20.4). The waiter thread
// waits N times on `word` with FUTEX_WAIT_PRIVATE until the main thread wakes it 2 ms after it
// said it was ready, counting its own calls and their results; then one futex_waitv on `word2`
// that times out after 1 ms; then one pthread_cond_wait the main thread signals after 5 ms.
// Prints "pid waiter_tid word_address word2_address", waits for a line, runs, prints its counts
// as JSON, and waits for a line before it exits.
//
//     futex_caller N
#define _GNU_SOURCE
#include <errno.h>
#include <linux/futex.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static _Atomic uint32_t word, word2, ready;
static int n;
static long calls, ok, eagain, waitv_ret = 1;
static pid_t waiter_tid;
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static int signalled, cond_waiting;
static pthread_barrier_t start;

static void sleep_ns(long ns)
{
  struct timespec t = {0, ns};
  nanosleep(&t, NULL);
}

static void * waiter(void * arg)
{
  (void)arg;
  waiter_tid = gettid();
  pthread_barrier_wait(&start);
  pthread_barrier_wait(&start);
  for (int i = 0; i < n; ++i) {
    atomic_store(&word, 0);
    atomic_store(&ready, 1);
    while (atomic_load(&word) == 0) {
      ++calls;
      if (syscall(SYS_futex, &word, FUTEX_WAIT_PRIVATE, 0, NULL, NULL, 0) == 0) {
        ++ok;
      } else if (errno == EAGAIN) {
        ++eagain;
      }
    }
  }
  struct timespec until;
  clock_gettime(CLOCK_MONOTONIC, &until);
  until.tv_nsec += 1000000;
  if (until.tv_nsec >= 1000000000) {
    until.tv_sec += 1;
    until.tv_nsec -= 1000000000;
  }
  struct futex_waitv w = {.val = 0, .uaddr = (uintptr_t)&word2, .flags = FUTEX_32};
  waitv_ret = syscall(SYS_futex_waitv, &w, 1, 0, &until, CLOCK_MONOTONIC) == 0 ? 0 : -errno;
  pthread_mutex_lock(&mutex);
  cond_waiting = 1;
  while (!signalled) {
    pthread_cond_wait(&cond, &mutex);
  }
  pthread_mutex_unlock(&mutex);
  return NULL;
}

int main(int argc, char ** argv)
{
  n = argc > 1 ? atoi(argv[1]) : 50;
  pthread_barrier_init(&start, NULL, 2);
  pthread_t t;
  pthread_create(&t, NULL, waiter, NULL);
  pthread_barrier_wait(&start);
  printf("%d %d %lu %lu\n", getpid(), waiter_tid, (unsigned long)&word, (unsigned long)&word2);
  fflush(stdout);
  char line[64];
  if (!fgets(line, sizeof line, stdin)) return 1;
  pthread_barrier_wait(&start);
  for (int i = 0; i < n; ++i) {
    while (!atomic_load(&ready)) sleep_ns(50000);
    atomic_store(&ready, 0);
    sleep_ns(2000000);
    atomic_store(&word, 1);
    syscall(SYS_futex, &word, FUTEX_WAKE_PRIVATE, 1, NULL, NULL, 0);
  }
  for (;;) {
    pthread_mutex_lock(&mutex);
    int waiting = cond_waiting;
    pthread_mutex_unlock(&mutex);
    if (waiting) break;
    sleep_ns(100000);
  }
  sleep_ns(5000000);
  pthread_mutex_lock(&mutex);
  signalled = 1;
  pthread_cond_signal(&cond);
  pthread_mutex_unlock(&mutex);
  pthread_join(t, NULL);
  printf(
    "{\"calls\": %ld, \"ok\": %ld, \"eagain\": %ld, \"waitv\": %ld}\n", calls, ok, eagain,
    waitv_ret);
  fflush(stdout);
  if (!fgets(line, sizeof line, stdin)) return 1;
  return 0;
}
