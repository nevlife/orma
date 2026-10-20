// A minimal reader of design 13 for the tests: waits for /dev/shm/orma.PID.*, marks itself the
// reader, arms the file and drains every ring until the writer's liveness lock is gone, then
// prints the tables. An exited slot is drained, printed and freed as it is found.
//
//     shm_reader PID
//
// Lines: A once armed, R slot tid event flags t_mono_ns body-hex, T slot gen tid kind src entry key
// count sum max, S slot gen tid state table_overflow depth_overflow ring_drops, and last H
// threads_overflow missing0 missing1 missing2 missing3.
#define _GNU_SOURCE
#include "orma_preload/orma_events.h"
#include "orma_preload/orma_shm.h"

#include <fcntl.h>
#include <glob.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

static orma_shm_header * header;

static orma_thread_slot * slot_at(uint32_t i)
{
  return (orma_thread_slot *)((uint8_t *)header + header->header_size +
                              (size_t)i * header->thread_size);
}

static int lock(int fd, int cmd, short type, off_t byte)
{
  struct flock fl = {.l_type = type, .l_whence = SEEK_SET, .l_start = byte, .l_len = 1};
  int r = fcntl(fd, cmd, &fl);
  return cmd == F_OFD_GETLK ? (r == 0 && fl.l_type != F_UNLCK) : r;
}

static void drain(uint32_t i, orma_thread_slot * s)
{
  const uint8_t * ring = (const uint8_t *)s + orma_ring_offset(header->table_entries);
  uint64_t head = __atomic_load_n(&s->ring_head, __ATOMIC_ACQUIRE);
  uint64_t at = s->ring_tail;
  while (at < head) {
    const orma_record * r = (const orma_record *)(ring + at % header->ring_bytes);
    if (r->event != ORMA_EV_PAD) {
      printf(
        "R %u %u %u %u %llu ", i, s->tid, r->event, r->flags, (unsigned long long)r->t_mono_ns);
      for (const uint8_t * b = (const uint8_t *)(r + 1); b < (const uint8_t *)r + r->len; b++) {
        printf("%02x", *b);
      }
      printf("\n");
    }
    at += r->len;
  }
  __atomic_store_n(&s->ring_tail, at, __ATOMIC_RELEASE);
}

static void dump(uint32_t i, orma_thread_slot * s, uint32_t state)
{
  const orma_table_entry * t = (const orma_table_entry *)(s + 1);
  for (uint32_t k = 0; k < header->table_entries; k++) {
    if (__atomic_load_n(&t[k].kind, __ATOMIC_ACQUIRE) == 0) continue;
    printf(
      "T %u %u %u %u %u %u %llu %llu %llu %llu\n", i, s->generation, s->tid, t[k].kind, t[k].src,
      t[k].entry, (unsigned long long)t[k].key, (unsigned long long)t[k].count,
      (unsigned long long)t[k].sum, (unsigned long long)t[k].max);
  }
  printf(
    "S %u %u %u %u %llu %llu %llu\n", i, s->generation, s->tid, state,
    (unsigned long long)s->table_overflow, (unsigned long long)s->depth_overflow,
    (unsigned long long)s->ring_drops);
}

static void pass(void)
{
  for (uint32_t i = 0; i < header->n_threads; i++) {
    orma_thread_slot * s = slot_at(i);
    uint32_t state = __atomic_load_n(&s->state, __ATOMIC_ACQUIRE);
    if (state == ORMA_THREAD_FREE) continue;
    drain(i, s);
    if (state == ORMA_THREAD_EXITED) {
      dump(i, s, state);
      __atomic_store_n(&s->state, ORMA_THREAD_FREE, __ATOMIC_RELEASE);
    }
  }
}

int main(int argc, char ** argv)
{
  if (argc != 2) return 2;
  char pattern[64];
  snprintf(pattern, sizeof pattern, "/dev/shm/orma.%s.*", argv[1]);
  glob_t g = {0};
  for (int tries = 0; glob(pattern, 0, NULL, &g) != 0; tries++) {
    if (tries == 20000) return 3;
    usleep(500);
  }
  int fd = open(g.gl_pathv[0], O_RDWR | O_NOFOLLOW | O_CLOEXEC);
  struct stat st;
  if (fd < 0 || lock(fd, F_OFD_SETLK, F_WRLCK, ORMA_LOCK_READER) != 0) return 4;
  // The writer sizes the file, takes its lock, then writes the magic last.
  while (fstat(fd, &st) == 0 && st.st_size == 0) usleep(100);
  header = mmap(NULL, (size_t)st.st_size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  while (__atomic_load_n(&header->magic, __ATOMIC_ACQUIRE) != ORMA_SHM_MAGIC) usleep(100);
  if ((size_t)st.st_size != header->header_size + (size_t)header->n_threads * header->thread_size) {
    return 5;
  }
  __atomic_fetch_add(&header->reader_gen, 1, __ATOMIC_RELAXED);
  __atomic_store_n(&header->armed, 1, __ATOMIC_RELAXED);
  printf("A\n");
  fflush(stdout);
  while (lock(fd, F_OFD_GETLK, F_WRLCK, ORMA_LOCK_ALIVE)) {
    pass();
    usleep(1000);
  }
  pass();
  for (uint32_t i = 0; i < header->n_threads; i++) {
    orma_thread_slot * s = slot_at(i);
    if (s->state != ORMA_THREAD_FREE) dump(i, s, s->state);
  }
  printf(
    "H %llu %llx %llx %llx %llx\n", (unsigned long long)header->threads_overflow,
    (unsigned long long)header->missing[0], (unsigned long long)header->missing[1],
    (unsigned long long)header->missing[2], (unsigned long long)header->missing[3]);
  return 0;
}
