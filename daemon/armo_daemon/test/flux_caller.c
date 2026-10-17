// Runs flux callbacks a known number of times, for accuracy.py to count. Each early callback
// dispatches the late channel's executor inside it, on the same thread. The early channel is
// announced, so armod names it by its key; the late one is not, so it is named by its signpost.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include "flux.h"

#define N 300
#define EARLY_BUSY_US 1000
#define LATE_BUSY_US 200
#define FP 0x0A4C

#define CHECK(c)                                                                      \
  do {                                                                                \
    if (!(c)) {                                                                       \
      fprintf(stderr, "%s:%d: %s (%s)\n", __FILE__, __LINE__, #c, flux_last_error()); \
      exit(1);                                                                        \
    }                                                                                 \
  } while (0)

typedef struct {
  flux_channel *sub;
  void (*callback)(void);
} source_ctx;

static flux_channel *late_pub;
static flux_executor *late_ex;

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
  CHECK(flux_channel_publish(late_pub, (const uint8_t *)"x", 1) == FLUX_PUBLISHED_OK);
  size_t n = 0;
  CHECK(flux_executor_dispatch(late_ex, &n) == FLUX_OK && n == 1);
}

static int32_t attach(void *ctx) { return ((source_ctx *)ctx)->sub != NULL; }

static int32_t deliver_one(void *ctx) {
  source_ctx *s = ctx;
  flux_view v = {0};
  if (!flux_channel_take(s->sub, &v)) return 0;
  s->callback();
  flux_view_release(&v);
  return 1;
}

static const flux_channel *channel(void *ctx) { return ((source_ctx *)ctx)->sub; }

static void open_pair(const char *name, flux_channel **pub, flux_channel **sub) {
  flux_memory_policy mem = {false, false};
  CHECK(flux_channel_create(name, 64, 4, 1, FP, mem, NULL, FLUX_NO_STAMP, pub) == FLUX_OK);
  CHECK(flux_channel_open(name, FP, mem, NULL, FLUX_NO_STAMP, sub) == FLUX_OK);
}

static flux_executor *executor(source_ctx *s) {
  flux_executor *ex = NULL;
  CHECK(flux_executor_new(&ex) == FLUX_OK);
  flux_source src = {s, attach, deliver_one, channel, NULL, NULL, s};
  CHECK(flux_executor_add(ex, src, 0) == FLUX_OK);
  return ex;
}

int main(void) {
  char early_key[64], late_key[64], early_name[256], late_name[256];
  size_t len = 0;
  snprintf(early_key, sizeof early_key, "/armo_test/f2_early_%d", (int)getpid());
  snprintf(late_key, sizeof late_key, "/armo_test/f2_late_%d", (int)getpid());
  CHECK(flux_signpost_name(early_key, FP, early_name, sizeof early_name, &len) == FLUX_OK);
  CHECK(flux_signpost_name(late_key, FP, late_name, sizeof late_name, &len) == FLUX_OK);

  flux_channel *early_pub, *early_sub, *late_sub;
  open_pair(early_name, &early_pub, &early_sub);
  flux_manifest_entry entry = {early_name, early_key, "flux_caller", false};
  flux_announcement *announced = NULL;
  CHECK(flux_announce(&entry, &announced) == FLUX_OK);
  source_ctx early_src = {early_sub, early};
  flux_executor *early_ex = executor(&early_src);
  printf("%s %s\n", early_key, late_name);
  fflush(stdout);
  char line[16];
  CHECK(fgets(line, sizeof line, stdin) != NULL);

  open_pair(late_name, &late_pub, &late_sub);
  source_ctx late_src = {late_sub, late};
  late_ex = executor(&late_src);
  for (int i = 0; i < N; i++) {
    CHECK(flux_channel_publish(early_pub, (const uint8_t *)"x", 1) == FLUX_PUBLISHED_OK);
    size_t n = 0;
    CHECK(flux_executor_dispatch(early_ex, &n) == FLUX_OK && n == 1);
  }
  printf("done\n");
  fflush(stdout);
  sleep(2);
  usleep(500000);

  flux_executor_free(late_ex);
  flux_executor_free(early_ex);
  flux_announcement_free(announced);
  flux_channel_free(late_sub);
  flux_channel_free(late_pub);
  flux_channel_free(early_sub);
  flux_channel_free(early_pub);
  shm_unlink(early_name);
  shm_unlink(late_name);
  return 0;
}
