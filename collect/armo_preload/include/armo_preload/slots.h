// The shared memory file of design 13 and its thread slots, kept the same way by both backends:
// libarmo_preload.so writes backend 1 with tables and rings, libarmo_lttng.so backend 2 with only
// each slot's fixed part, where armod reads cur_cb (15.2). Each library links its own copy, so the
// two files of a process that loads both stay apart.
#ifndef ARMO_PRELOAD_SLOTS_H
#define ARMO_PRELOAD_SLOTS_H

#include "armo_preload/armo_shm.h"

#include <stdbool.h>
#include <stdint.h>

#pragma GCC visibility push(hidden)

#define ARMO_DEPTH 16
#define ARMO_SLOT_DEAD ((armo_thread_slot *)-1)
#define ARMO_LOAD(p) __atomic_load_n(p, __ATOMIC_RELAXED)
#define ARMO_STORE(p, v) __atomic_store_n(p, v, __ATOMIC_RELAXED)
#define ARMO_BUMP(p) ARMO_STORE(p, ARMO_LOAD(p) + 1)

struct armo_frame
{
  uint64_t key;
  uint64_t t0;
  uint32_t entry;
  uint16_t src;
};

// The last wake of a source kind on this thread; a callback at the same depth (and for flux of
// the same executor) measures its dispatch from it. Backend 1 only.
struct armo_wake
{
  uint64_t t;
  uint64_t executor;
  uint32_t depth;
};

struct armo_thread
{
  armo_thread_slot * slot;
  struct armo_wake wake[ARMO_SRC_FLUX + 1];
  uint64_t wait_start[ARMO_SRC_FLUX + 1];
  uint32_t n;
  uint32_t untracked;
  int busy;
  struct armo_frame stack[ARMO_DEPTH];
};

// What the backend's file holds; each backend defines it. `suffix` follows the file name
// armo.<pid>.<starttime>.
struct armo_layout
{
  uint32_t backend;
  uint32_t table_entries;
  uint32_t ring_bytes;
  const char * suffix;
};

extern const struct armo_layout armo_layout;
extern __thread struct armo_thread armo_self __attribute__((tls_model("initial-exec")));
extern armo_shm_header * armo_header;

// The process's file, made on the first call; NULL if it could not be.
armo_shm_header * armo_file(void);
struct armo_thread * armo_thread_claim(void);

static inline struct armo_thread * armo_thread(void)
{
  armo_thread_slot * s = armo_self.slot;
  if (s != NULL) return s == ARMO_SLOT_DEAD ? NULL : &armo_self;
  return armo_thread_claim();
}

// An extra collection the reader asked for in the header's options; off before the file exists.
static inline bool armo_option(uint32_t bit)
{
  armo_shm_header * h = __atomic_load_n(&armo_header, __ATOMIC_ACQUIRE);
  return h != NULL && (ARMO_LOAD(&h->options) & bit) != 0;
}

static inline void armo_set_current(struct armo_thread * t)
{
  armo_thread_slot * s = t->slot;
  const struct armo_frame * top = t->n > 0 ? &t->stack[t->n - 1] : NULL;
  ARMO_STORE(&s->depth, t->n + t->untracked);
  ARMO_STORE(&s->cur_src, top != NULL ? top->src : 0);
  ARMO_STORE(&s->cur_entry, top != NULL ? top->entry : 0);
  ARMO_STORE(&s->cur_cb, top != NULL ? top->key : 0);
}

static inline void armo_push(
  struct armo_thread * t, uint16_t src, uint64_t key, uint32_t entry, uint64_t t0)
{
  if (t->n < ARMO_DEPTH) {
    t->stack[t->n++] = (struct armo_frame){.key = key, .t0 = t0, .entry = entry, .src = src};
  } else {
    t->untracked++;
    ARMO_BUMP(&t->slot->depth_overflow);
  }
  armo_set_current(t);
}

// Ends the innermost open callback with this key; false if it was not on the stack. Python
// coroutine callbacks can end out of order, so the match searches down the stack.
static inline bool armo_pop(
  struct armo_thread * t, uint16_t src, uint64_t key, uint32_t entry, uint64_t * t0)
{
  uint32_t i = t->n;
  while (i > 0 && !(t->stack[i - 1].key == key && t->stack[i - 1].src == src &&
                    t->stack[i - 1].entry == entry)) {
    i--;
  }
  bool found = i > 0;
  if (found) {
    *t0 = t->stack[i - 1].t0;
    for (; i < t->n; i++) t->stack[i - 1] = t->stack[i];
    t->n--;
  } else if (t->untracked > 0) {
    t->untracked--;
  }
  armo_set_current(t);
  return found;
}

#pragma GCC visibility pop

#endif  // ARMO_PRELOAD_SLOTS_H
