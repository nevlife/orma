// flux_trace_* of trace contract 2 (design 11.2). The events are emitted only when a loaded
// library exports flux_trace_contract_2; the call goes on to the next definition either way.
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"

#include <stdbool.h>
#include <stdio.h>

#define CONTRACT "flux_trace_contract_2"

// 0 not checked yet, 1 a loaded flux library has the contract, -1 none has. Not asked of the
// caller: a tail call hides it. Nor of the next definition, which can be another orma backend.
static int contract;

static bool agreed(void)
{
  int c = __atomic_load_n(&contract, __ATOMIC_RELAXED);
  if (c != 0) {
    return c > 0;
  }
  c = orma_find(CONTRACT) != NULL ? 1 : -1;
  int unset = 0;
  if (
    __atomic_compare_exchange_n(&contract, &unset, c, false, __ATOMIC_RELAXED, __ATOMIC_RELAXED) &&
    c < 0) {
    fprintf(stderr, "orma_preload: the flux library lacks %s; no flux events\n", CONTRACT);
  }
  return c > 0;
}

#define FLUX_POINT(name, event, params, args)                       \
  static void emit_##name params;                                   \
  ORMA_EXPORT void flux_trace_##name params;                        \
  void flux_trace_##name params                                     \
  {                                                                 \
    void(*next) params = ORMA_NEXT(flux_trace_##name, NULL, event); \
    if (agreed()) emit_##name args;                                 \
    if (next != NULL) next args;                                    \
  }                                                                 \
  static void emit_##name params

FLUX_POINT(
  channel_init, ORMA_EV_FLUX_CHANNEL_INIT,
  (const void * channel, uint64_t dev, uint64_t ino, const char * segment, int32_t publisher,
   uint32_t slot_count, uint32_t slot_size, int32_t stamp_offset),
  (channel, dev, ino, segment, publisher, slot_count, slot_size, stamp_offset))
{
  orma_ev_flux_channel_init ev = {
    .channel = ORMA_U64(channel),
    .dev = dev,
    .ino = ino,
    .publisher = publisher,
    .slot_count = slot_count,
    .slot_size = slot_size,
    .stamp_offset = stamp_offset,
  };
  orma_emit_flux_channel_init(&ev, segment);
}

FLUX_POINT(
  claim, ORMA_EV_FLUX_CLAIM, (const void * channel, uint32_t slot, int32_t result, int32_t loan),
  (channel, slot, result, loan))
{
  orma_ev_flux_claim ev = {
    .channel = ORMA_U64(channel), .slot = slot, .result = result, .loan = loan};
  orma_emit_flux_claim(&ev);
}

FLUX_POINT(loan, ORMA_EV_FLUX_LOAN, (const void * channel, uint32_t slot), (channel, slot))
{
  orma_ev_flux_loan ev = {.channel = ORMA_U64(channel), .slot = slot};
  orma_emit_flux_loan(&ev);
}

FLUX_POINT(
  loan_end, ORMA_EV_FLUX_LOAN_END, (const void * channel, uint32_t slot, int32_t result),
  (channel, slot, result))
{
  orma_ev_flux_loan_end ev = {.channel = ORMA_U64(channel), .slot = slot, .result = result};
  orma_emit_flux_loan_end(&ev);
}

FLUX_POINT(
  commit, ORMA_EV_FLUX_COMMIT,
  (const void * channel, uint64_t ticket, uint32_t slot, uint64_t nbytes, int32_t woke,
   uint64_t stamp),
  (channel, ticket, slot, nbytes, woke, stamp))
{
  orma_ev_flux_commit ev = {
    .channel = ORMA_U64(channel),
    .ticket = ticket,
    .nbytes = nbytes,
    .stamp = stamp,
    .slot = slot,
    .woke = woke,
  };
  orma_emit_flux_commit(&ev);
}

FLUX_POINT(
  take, ORMA_EV_FLUX_TAKE,
  (const void * channel, uint64_t ticket, uint32_t slot, uint64_t lost, uint64_t stamp),
  (channel, ticket, slot, lost, stamp))
{
  orma_ev_flux_take ev = {
    .channel = ORMA_U64(channel), .ticket = ticket, .lost = lost, .stamp = stamp, .slot = slot};
  orma_emit_flux_take(&ev);
}

FLUX_POINT(
  release, ORMA_EV_FLUX_RELEASE, (const void * channel, uint64_t ticket, int32_t held),
  (channel, ticket, held))
{
  orma_ev_flux_release ev = {.channel = ORMA_U64(channel), .ticket = ticket, .held = held};
  orma_emit_flux_release(&ev);
}

FLUX_POINT(refused, ORMA_EV_FLUX_REFUSED, (const void * channel, int32_t reason), (channel, reason))
{
  orma_ev_flux_refused ev = {.channel = ORMA_U64(channel), .reason = reason};
  orma_emit_flux_refused(&ev);
}

FLUX_POINT(
  wait, ORMA_EV_FLUX_WAIT, (const void * executor, uint32_t sources, int64_t timeout_ns),
  (executor, sources, timeout_ns))
{
  orma_ev_flux_wait ev = {
    .executor = ORMA_U64(executor), .timeout_ns = timeout_ns, .sources = sources};
  orma_emit_flux_wait(&ev);
}

FLUX_POINT(
  wake, ORMA_EV_FLUX_WAKE, (const void * executor, int32_t result, uint32_t entry),
  (executor, result, entry))
{
  orma_ev_flux_wake ev = {.executor = ORMA_U64(executor), .result = result, .entry = entry};
  orma_emit_flux_wake(&ev);
}

FLUX_POINT(
  callback_start, ORMA_EV_FLUX_CALLBACK_START,
  (const void * executor, uint32_t entry, const void * endpoint), (executor, entry, endpoint))
{
  orma_ev_flux_callback_start ev = {
    .executor = ORMA_U64(executor), .endpoint = ORMA_U64(endpoint), .entry = entry};
  orma_emit_flux_callback_start(&ev);
}

FLUX_POINT(
  callback_end, ORMA_EV_FLUX_CALLBACK_END,
  (const void * executor, uint32_t entry, int32_t delivered, const void * channel,
   const char * segment),
  (executor, entry, delivered, channel, segment))
{
  orma_ev_flux_callback_end ev = {
    .executor = ORMA_U64(executor),
    .channel = ORMA_U64(channel),
    .entry = entry,
    .delivered = delivered,
  };
  orma_emit_flux_callback_end(&ev, segment);
}

FLUX_POINT(
  endpoint_init, ORMA_EV_FLUX_ENDPOINT_INIT,
  (const void * endpoint, const char * node, const char * key, const char * signpost,
   uint64_t fingerprint, int32_t role, int32_t lang),
  (endpoint, node, key, signpost, fingerprint, role, lang))
{
  orma_ev_flux_endpoint_init ev = {
    .endpoint = ORMA_U64(endpoint), .fingerprint = fingerprint, .role = role, .lang = lang};
  orma_emit_flux_endpoint_init(&ev, node, key, signpost);
}

FLUX_POINT(py_gil, ORMA_EV_FLUX_PY_GIL, (int32_t phase), (phase))
{
  orma_ev_flux_py_gil ev = {.phase = phase};
  orma_emit_flux_py_gil(&ev);
}

FLUX_POINT(
  py_ready, ORMA_EV_FLUX_PY_READY, (const void * channel, uint64_t ticket), (channel, ticket))
{
  orma_ev_flux_py_ready ev = {.channel = ORMA_U64(channel), .ticket = ticket};
  orma_emit_flux_py_ready(&ev);
}

FLUX_POINT(
  record_drop, ORMA_EV_FLUX_RECORD_DROP,
  (const void * channel, uint64_t ticket, uint32_t count, int32_t reason),
  (channel, ticket, count, reason))
{
  orma_ev_flux_record_drop ev = {
    .channel = ORMA_U64(channel), .ticket = ticket, .count = count, .reason = reason};
  orma_emit_flux_record_drop(&ev);
}

FLUX_POINT(gpu_wedged, ORMA_EV_FLUX_GPU_WEDGED, (int32_t result), (result))
{
  orma_ev_flux_gpu_wedged ev = {.result = result};
  orma_emit_flux_gpu_wedged(&ev);
}
