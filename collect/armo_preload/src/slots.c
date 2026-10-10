// The file appears on the first call that needs it and a thread takes a slot on its first event;
// each slot is written only by its thread (design 13.1, 13.3).
#define _GNU_SOURCE
#include "armo_preload/slots.h"

#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

__thread struct armo_thread armo_self __attribute__((tls_model("initial-exec")));
armo_shm_header * armo_header;

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
  for (int field = 3; field <= 22; field++) p = strchr(p + 1, ' ');
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
  return lock_byte(proc.fd, F_OFD_GETLK, F_WRLCK, ARMO_LOCK_READER, &fl) == 0 &&
         fl.l_type != F_UNLCK;
}

static armo_thread_slot * slot_at(uint32_t i)
{
  return (armo_thread_slot *)((uint8_t *)armo_header + ARMO_SHM_HEADER_SIZE + i * proc.thread_size);
}

static void thread_exit(void * arg)
{
  (void)arg;
  armo_thread_slot * s = armo_self.slot;
  if (s == NULL || s == ARMO_SLOT_DEAD) return;
  armo_self.slot = ARMO_SLOT_DEAD;
  __atomic_store_n(&s->state, ARMO_THREAD_EXITED, __ATOMIC_RELEASE);
}

static void remove_file(void)
{
  if (proc.path[0] != '\0' && getpid() == proc.pid) unlink(proc.path);
}

// A child of fork without exec would otherwise write its parent's slots and, at exit, remove
// its parent's file. It starts over with a file of its own.
static void forked(void)
{
  if (armo_header != NULL) {
    munmap(armo_header, proc.size);
    close(proc.fd);
  }
  armo_header = NULL;
  proc.path[0] = '\0';
  failed = 0;
  armo_self.slot = NULL;
  pthread_mutex_init(&creating, NULL);
}

static armo_shm_header * create(void)
{
  const char * env = getenv("ARMO_THREADS");
  proc.n_threads = env != NULL ? (uint32_t)strtoul(env, NULL, 10) : ARMO_THREADS_DEFAULT;
  proc.thread_size = armo_thread_size(armo_layout.table_entries, armo_layout.ring_bytes);
  proc.size = ARMO_SHM_HEADER_SIZE + (size_t)proc.n_threads * proc.thread_size;
  proc.pid = getpid();
  uint64_t start = starttime();
  snprintf(
    proc.path, sizeof proc.path, "/dev/shm/armo.%d.%llu%s", (int)proc.pid,
    (unsigned long long)start, armo_layout.suffix);

  int fd = open(proc.path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
  if (fd < 0) {
    proc.path[0] = '\0';
    return NULL;
  }
  struct flock fl;
  void * map = MAP_FAILED;
  if (
    ftruncate(fd, (off_t)proc.size) == 0 &&
    lock_byte(fd, F_OFD_SETLK, F_RDLCK, ARMO_LOCK_ALIVE, &fl) == 0) {
    map = mmap(NULL, proc.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  }
  if (map == MAP_FAILED) {
    unlink(proc.path);
    close(fd);
    proc.path[0] = '\0';
    return NULL;
  }
  proc.fd = fd;
  armo_shm_header * h = map;
  h->version = ARMO_SHM_VERSION;
  h->header_size = ARMO_SHM_HEADER_SIZE;
  h->thread_size = (uint32_t)proc.thread_size;
  h->n_threads = proc.n_threads;
  h->ring_bytes = armo_layout.ring_bytes;
  h->table_entries = armo_layout.table_entries;
  h->backend = armo_layout.backend;
  h->pid = (uint32_t)proc.pid;
  h->starttime = start;
  __atomic_store_n(&h->magic, ARMO_SHM_MAGIC, __ATOMIC_RELEASE);

  if (!registered) {
    registered = 1;
    pthread_key_create(&proc.key, thread_exit);
    atexit(remove_file);
    pthread_atfork(NULL, NULL, forked);
  }
  return h;
}

armo_shm_header * armo_file(void)
{
  armo_shm_header * h = __atomic_load_n(&armo_header, __ATOMIC_ACQUIRE);
  if (h != NULL || ARMO_LOAD(&failed)) return h;
  pthread_mutex_lock(&creating);
  h = armo_header;
  if (h == NULL && !failed) {
    h = create();
    if (h != NULL) {
      __atomic_store_n(&armo_header, h, __ATOMIC_RELEASE);
    } else {
      ARMO_STORE(&failed, 1);
    }
  }
  pthread_mutex_unlock(&creating);
  return h;
}

// Ring positions are running byte counts; a reclaimed slot nobody reads starts with an empty ring.
static struct armo_thread * take(armo_thread_slot * s, bool reclaim)
{
  memset(s + 1, 0, armo_layout.table_entries * sizeof(armo_table_entry));
  ARMO_STORE(&s->depth, 0);
  ARMO_STORE(&s->cur_cb, 0);
  ARMO_STORE(&s->cur_src, 0);
  ARMO_STORE(&s->cur_entry, 0);
  ARMO_STORE(&s->table_overflow, 0);
  ARMO_STORE(&s->depth_overflow, 0);
  ARMO_STORE(&s->ring_drops, 0);
  if (reclaim) ARMO_STORE(&s->ring_tail, ARMO_LOAD(&s->ring_head));
  // The ring's pages, faulted in now rather than one shmem fault per page as the ring first
  // fills (64 for 256 KiB). Before 5.14 the kernel refuses the advice and they fault as before.
  uintptr_t page = (uintptr_t)sysconf(_SC_PAGESIZE);
  uintptr_t from = (uintptr_t)s & ~(page - 1);
  madvise((void *)from, (uintptr_t)s + proc.thread_size - from, MADV_POPULATE_WRITE);
  ARMO_STORE(&s->tid, (uint32_t)gettid());
  __atomic_fetch_add(&s->generation, 1, __ATOMIC_RELEASE);
  memset(&armo_self, 0, sizeof armo_self);
  armo_self.slot = s;
  armo_self.busy = 1;
  pthread_setspecific(proc.key, &armo_self);
  return &armo_self;
}

static struct armo_thread * claim(armo_shm_header * h)
{
  for (int pass = 0; pass < 2; pass++) {
    uint32_t from = pass == 0 ? ARMO_THREAD_FREE : ARMO_THREAD_EXITED;
    if (pass == 1 && reader_present()) break;
    for (uint32_t i = 0; i < proc.n_threads; i++) {
      armo_thread_slot * s = slot_at(i);
      uint32_t state = from;
      if (__atomic_compare_exchange_n(
            &s->state, &state, ARMO_THREAD_LIVE, false, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        return take(s, pass == 1);
      }
    }
  }
  __atomic_fetch_add(&h->threads_overflow, 1, __ATOMIC_RELAXED);
  return NULL;
}

struct armo_thread * armo_thread_claim(void)
{
  if (armo_self.busy) return NULL;
  armo_self.busy = 1;
  armo_shm_header * h = armo_file();
  struct armo_thread * t = h != NULL ? claim(h) : NULL;
  if (t == NULL) armo_self.slot = ARMO_SLOT_DEAD;
  armo_self.busy = 0;
  return t;
}
