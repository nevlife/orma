// The file appears on the first call that needs it and a thread takes a slot on its first event;
// each slot is written only by its thread (design 13.1, 13.3).
#define _GNU_SOURCE
#include "orma_preload/slots.h"

#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

__thread struct orma_thread orma_self __attribute__((tls_model("initial-exec")));
orma_shm_header * orma_header;

static struct
{
  size_t size;
  size_t thread_size;
  uint32_t n_threads;
  int fd;
  pid_t pid;
  pthread_key_t key;
  char path[64];
} proc;

static int failed;
static int registered;
static pthread_mutex_t creating = PTHREAD_MUTEX_INITIALIZER;

// Field 22 of /proc/self/stat, counted after the command name's closing parenthesis.
static uint64_t starttime(void)
{
  char buf[1024];
  int fd = open("/proc/self/stat", O_RDONLY | O_CLOEXEC);
  ssize_t n = read(fd, buf, sizeof buf - 1);
  close(fd);
  buf[n] = '\0';
  char * p = strrchr(buf, ')');
  for (int field = 3; field <= 22; field++) {
    p = strchr(p + 1, ' ');
  }
  return strtoull(p + 1, NULL, 10);
}

static int lock_byte(int fd, int cmd, short type, off_t byte, struct flock * fl)
{
  *fl = (struct flock){.l_type = type, .l_whence = SEEK_SET, .l_start = byte, .l_len = 1};
  return fcntl(fd, cmd, fl);
}

static bool reader_present(void)
{
  struct flock fl;
  return lock_byte(proc.fd, F_OFD_GETLK, F_WRLCK, ORMA_LOCK_READER, &fl) == 0 &&
         fl.l_type != F_UNLCK;
}

static orma_thread_slot * slot_at(uint32_t i)
{
  return (orma_thread_slot *)((uint8_t *)orma_header + ORMA_SHM_HEADER_SIZE + i * proc.thread_size);
}

static void thread_exit(void * arg)
{
  (void)arg;
  orma_thread_slot * s = orma_self.slot;
  if (s == NULL || s == ORMA_SLOT_DEAD) {
    return;
  }
  orma_self.slot = ORMA_SLOT_DEAD;
  __atomic_store_n(&s->state, ORMA_THREAD_EXITED, __ATOMIC_RELEASE);
}

static void remove_file(void)
{
  if (proc.path[0] != '\0' && getpid() == proc.pid) {
    unlink(proc.path);
  }
}

// A child of fork without exec would otherwise write its parent's slots and, at exit, remove
// its parent's file. It starts over with a file of its own.
static void forked(void)
{
  if (orma_header != NULL) {
    munmap(orma_header, proc.size);
    close(proc.fd);
  }
  orma_header = NULL;
  proc.path[0] = '\0';
  failed = 0;
  orma_self.slot = NULL;
  pthread_mutex_init(&creating, NULL);
}

static orma_shm_header * create(void)
{
  const char * env = getenv("ORMA_THREADS");
  proc.n_threads = env != NULL ? (uint32_t)strtoul(env, NULL, 10) : ORMA_THREADS_DEFAULT;
  proc.thread_size = orma_thread_size(orma_layout.table_entries, orma_layout.ring_bytes);
  proc.size = ORMA_SHM_HEADER_SIZE + (size_t)proc.n_threads * proc.thread_size;
  proc.pid = getpid();
  uint64_t start = starttime();
  snprintf(
    proc.path, sizeof proc.path, "/dev/shm/orma.%d.%llu%s", (int)proc.pid,
    (unsigned long long)start, orma_layout.suffix);

  int fd = open(proc.path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
  if (fd < 0) {
    proc.path[0] = '\0';
    return NULL;
  }
  struct flock fl;
  void * map = MAP_FAILED;
  if (
    ftruncate(fd, (off_t)proc.size) == 0 &&
    lock_byte(fd, F_OFD_SETLK, F_RDLCK, ORMA_LOCK_ALIVE, &fl) == 0) {
    map = mmap(NULL, proc.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  }
  if (map == MAP_FAILED) {
    unlink(proc.path);
    close(fd);
    proc.path[0] = '\0';
    return NULL;
  }
  proc.fd = fd;
  orma_shm_header * h = map;
  h->version = ORMA_SHM_VERSION;
  h->header_size = ORMA_SHM_HEADER_SIZE;
  h->thread_size = (uint32_t)proc.thread_size;
  h->n_threads = proc.n_threads;
  h->ring_bytes = orma_layout.ring_bytes;
  h->table_entries = orma_layout.table_entries;
  h->backend = orma_layout.backend;
  h->pid = (uint32_t)proc.pid;
  h->starttime = start;
  __atomic_store_n(&h->magic, ORMA_SHM_MAGIC, __ATOMIC_RELEASE);

  if (!registered) {
    registered = 1;
    pthread_key_create(&proc.key, thread_exit);
    atexit(remove_file);
    pthread_atfork(NULL, NULL, forked);
  }
  return h;
}

orma_shm_header * orma_file(void)
{
  orma_shm_header * h = __atomic_load_n(&orma_header, __ATOMIC_ACQUIRE);
  if (h != NULL || ORMA_LOAD(&failed)) {
    return h;
  }
  pthread_mutex_lock(&creating);
  h = orma_header;
  if (h == NULL && !failed) {
    h = create();
    if (h != NULL) {
      __atomic_store_n(&orma_header, h, __ATOMIC_RELEASE);
    } else {
      ORMA_STORE(&failed, 1);
    }
  }
  pthread_mutex_unlock(&creating);
  return h;
}

// Ring positions are running byte counts; a reclaimed slot nobody reads starts with an empty ring.
static struct orma_thread * take(orma_thread_slot * s, bool reclaim)
{
  memset(s + 1, 0, orma_layout.table_entries * sizeof(orma_table_entry));
  ORMA_STORE(&s->depth, 0);
  ORMA_STORE(&s->cur_cb, 0);
  ORMA_STORE(&s->cur_src, 0);
  ORMA_STORE(&s->cur_entry, 0);
  ORMA_STORE(&s->table_overflow, 0);
  ORMA_STORE(&s->depth_overflow, 0);
  ORMA_STORE(&s->ring_drops, 0);
  if (reclaim) {
    ORMA_STORE(&s->ring_tail, ORMA_LOAD(&s->ring_head));
  }
  // The ring's pages, faulted in now rather than one shmem fault per page as the ring first
  // fills (64 for 256 KiB). Before 5.14 the kernel refuses the advice and they fault as before.
  uintptr_t page = (uintptr_t)sysconf(_SC_PAGESIZE);
  uintptr_t from = (uintptr_t)s & ~(page - 1);
  madvise((void *)from, (uintptr_t)s + proc.thread_size - from, MADV_POPULATE_WRITE);
  ORMA_STORE(&s->tid, (uint32_t)gettid());
  __atomic_fetch_add(&s->generation, 1, __ATOMIC_RELEASE);
  memset(&orma_self, 0, sizeof orma_self);
  orma_self.slot = s;
  orma_self.busy = 1;
  pthread_setspecific(proc.key, &orma_self);
  return &orma_self;
}

static struct orma_thread * claim(orma_shm_header * h)
{
  for (int pass = 0; pass < 2; pass++) {
    uint32_t from = pass == 0 ? ORMA_THREAD_FREE : ORMA_THREAD_EXITED;
    if (pass == 1 && reader_present()) {
      break;
    }
    for (uint32_t i = 0; i < proc.n_threads; i++) {
      orma_thread_slot * s = slot_at(i);
      uint32_t state = from;
      if (__atomic_compare_exchange_n(
            &s->state, &state, ORMA_THREAD_LIVE, false, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        return take(s, pass == 1);
      }
    }
  }
  __atomic_fetch_add(&h->threads_overflow, 1, __ATOMIC_RELAXED);
  return NULL;
}

struct orma_thread * orma_thread_claim(void)
{
  if (orma_self.busy) {
    return NULL;
  }
  orma_self.busy = 1;
  orma_shm_header * h = orma_file();
  struct orma_thread * t = h != NULL ? claim(h) : NULL;
  if (t == NULL) {
    orma_self.slot = ORMA_SLOT_DEAD;
  }
  orma_self.busy = 0;
  return t;
}
