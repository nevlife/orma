// The shared memory file of design 13 and its thread slots, kept the same way by both backends:
// liborma_preload.so writes backend 1 with tables and rings, liborma_lttng.so backend 2 with only
// each slot's fixed part, where ormad reads cur_cb (15.2). Each library links its own copy, so the
// two files of a process that loads both stay apart.
#ifndef ORMA_PRELOAD_SLOTS_H
#define ORMA_PRELOAD_SLOTS_H

#include "orma_preload/orma_shm.h"

#include <stdbool.h>
#include <stdint.h>

#pragma GCC visibility push(hidden)

#define ORMA_DEPTH 16
#define ORMA_SLOT_DEAD ((orma_thread_slot *)-1)
#define ORMA_LOAD(p) __atomic_load_n(p, __ATOMIC_RELAXED)
#define ORMA_STORE(p, v) __atomic_store_n(p, v, __ATOMIC_RELAXED)
#define ORMA_BUMP(p) ORMA_STORE(p, ORMA_LOAD(p) + 1)

struct orma_frame
{
  uint64_t key;
  uint64_t t0;
  uint32_t entry;
  uint16_t src;
};

// The last wake of a source kind on this thread; a callback at the same depth (and for flux of
// the same executor) measures its dispatch from it. Backend 1 only.
struct orma_wake
{
  uint64_t t;
  uint64_t executor;
  uint32_t depth;
};

struct orma_thread
{
  orma_thread_slot * slot;
  struct orma_wake wake[ORMA_SRC_FLUX + 1];
  uint64_t wait_start[ORMA_SRC_FLUX + 1];
  uint32_t n;
  uint32_t untracked;
  int busy;
  struct orma_frame stack[ORMA_DEPTH];
};

// What the backend's file holds; each backend defines it. `suffix` follows the file name
// orma.<pid>.<starttime>.
struct orma_layout
{
  uint32_t backend;
  uint32_t table_entries;
  uint32_t ring_bytes;
  const char * suffix;
};

extern const struct orma_layout orma_layout;
extern __thread struct orma_thread orma_self __attribute__((tls_model("initial-exec")));
extern orma_shm_header * orma_header;

// The process's file, made on the first call; NULL if it could not be.
orma_shm_header * orma_file(void);
struct orma_thread * orma_thread_claim(void);

static inline struct orma_thread * orma_thread(void)
{
  orma_thread_slot * s = orma_self.slot;
  if (s != NULL) return s == ORMA_SLOT_DEAD ? NULL : &orma_self;
  return orma_thread_claim();
}

// An extra collection the reader asked for in the header's options; off before the file exists.
static inline bool orma_option(uint32_t bit)
{
  orma_shm_header * h = __atomic_load_n(&orma_header, __ATOMIC_ACQUIRE);
  return h != NULL && (ORMA_LOAD(&h->options) & bit) != 0;
}

static inline void orma_set_current(struct orma_thread * t)
{
  orma_thread_slot * s = t->slot;
  const struct orma_frame * top = t->n > 0 ? &t->stack[t->n - 1] : NULL;
  ORMA_STORE(&s->depth, t->n + t->untracked);
  ORMA_STORE(&s->cur_src, top != NULL ? top->src : 0);
  ORMA_STORE(&s->cur_entry, top != NULL ? top->entry : 0);
  ORMA_STORE(&s->cur_cb, top != NULL ? top->key : 0);
}

static inline void orma_push(
  struct orma_thread * t, uint16_t src, uint64_t key, uint32_t entry, uint64_t t0)
{
  if (t->n < ORMA_DEPTH) {
    t->stack[t->n++] = (struct orma_frame){.key = key, .t0 = t0, .entry = entry, .src = src};
  } else {
    t->untracked++;
    ORMA_BUMP(&t->slot->depth_overflow);
  }
  orma_set_current(t);
}

// Ends the innermost open callback with this key; false if it was not on the stack. Python
// coroutine callbacks can end out of order, so the match searches down the stack.
static inline bool orma_pop(
  struct orma_thread * t, uint16_t src, uint64_t key, uint32_t entry, uint64_t * t0)
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
  orma_set_current(t);
  return found;
}

#pragma GCC visibility pop

#endif  // ORMA_PRELOAD_SLOTS_H
