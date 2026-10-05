// Runs flux2 callbacks a known number of times, for accuracy.py to count. Each early callback
// dispatches the late channel's executor inside it, on the same thread. The early channel is
// announced, so armod names it by its key; the late one is not, so it is named by its signpost.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include "flux2.h"

#define N 300
#define EARLY_BUSY_US 1000
#define LATE_BUSY_US 200
#define FP 0x0A4C

#define CHECK(c)                                                                      \
  do {                                                                                \
    if (!(c)) {                                                                       \
      fprintf(stderr, "%s:%d: %s (%s)\n", __FILE__, __LINE__, #c, flux2_last_error()); \
      exit(1);                                                                        \
    }                                                                                 \
  } while (0)

typedef struct {
  flux2_channel *sub;
  void (*callback)(void);
} source_ctx;

static flux2_channel *late_pub;
static flux2_executor *late_ex;

static void busy(long us) {
  struct timespec t0, t;
  clock_gettime(CLOCK_MONOTONIC, &t0);
  do {
    clock_gettime(CLOCK_MONOTONIC, &t);
  } while ((t.tv_sec - t0.tv_sec) * 1000000L + (t.tv_nsec - t0.tv_nsec) / 1000 < us);
}

static void late(void) { busy(LATE_BUSY_US); }

static void early(void) {
  busy(EARLY_BUSY_US);
  CHECK(flux2_channel_publish(late_pub, (const uint8_t *)"x", 1) == FLUX2_PUBLISHED_OK);
  size_t n = 0;
  CHECK(flux2_executor_dispatch(late_ex, &n) == FLUX2_OK && n == 1);
}

static bool attach(void *ctx) { return ((source_ctx *)ctx)->sub != NULL; }

static int32_t deliver_one(void *ctx) {
  source_ctx *s = ctx;
  flux2_view v = {0};
  if (!flux2_channel_take(s->sub, &v)) return 0;
  s->callback();
  flux2_view_release(&v);
  return 1;
}

static const flux2_channel *channel(void *ctx) { return ((source_ctx *)ctx)->sub; }

static void open_pair(const char *name, flux2_channel **pub, flux2_channel **sub) {
  flux2_memory_policy mem = {false, false};
  CHECK(flux2_channel_create(name, 64, 4, FP, mem, pub) == FLUX2_OK);
  CHECK(flux2_channel_open(name, FP, mem, sub) == FLUX2_OK);
}

static flux2_executor *executor(source_ctx *s) {
  flux2_executor *ex = NULL;
  CHECK(flux2_executor_new(&ex) == FLUX2_OK);
  flux2_source src = {s, attach, deliver_one, channel, NULL};
  CHECK(flux2_executor_add(ex, src, 0) == FLUX2_OK);
  return ex;
}

int main(void) {
  char early_key[64], late_key[64], early_name[256], late_name[256];
  size_t len = 0;
  snprintf(early_key, sizeof early_key, "/armo_test/f2_early_%d", (int)getpid());
  snprintf(late_key, sizeof late_key, "/armo_test/f2_late_%d", (int)getpid());
  CHECK(flux2_signpost_name(early_key, FP, early_name, sizeof early_name, &len) == FLUX2_OK);
  CHECK(flux2_signpost_name(late_key, FP, late_name, sizeof late_name, &len) == FLUX2_OK);

  flux2_channel *early_pub, *early_sub, *late_sub;
  open_pair(early_name, &early_pub, &early_sub);
  flux2_manifest_entry entry = {early_name, early_key, "flux2_caller", false};
  flux2_announcement *announced = NULL;
  CHECK(flux2_announce(&entry, &announced) == FLUX2_OK);
  source_ctx early_src = {early_sub, early};
  flux2_executor *early_ex = executor(&early_src);
  printf("%s %s\n", early_key, late_name);
  fflush(stdout);
  char line[16];
  CHECK(fgets(line, sizeof line, stdin) != NULL);

  open_pair(late_name, &late_pub, &late_sub);
  source_ctx late_src = {late_sub, late};
  late_ex = executor(&late_src);
  for (int i = 0; i < N; i++) {
    CHECK(flux2_channel_publish(early_pub, (const uint8_t *)"x", 1) == FLUX2_PUBLISHED_OK);
    size_t n = 0;
    CHECK(flux2_executor_dispatch(early_ex, &n) == FLUX2_OK && n == 1);
  }
  printf("done\n");
  fflush(stdout);
  sleep(2);
  usleep(500000);

  flux2_executor_free(late_ex);
  flux2_executor_free(early_ex);
  flux2_announcement_free(announced);
  flux2_channel_free(late_sub);
  flux2_channel_free(late_pub);
  flux2_channel_free(early_sub);
  flux2_channel_free(early_pub);
  shm_unlink(early_name);
  shm_unlink(late_name);
  return 0;
}
