// flux2_trace_* of trace contract 2 (design 11.2). The events are emitted only when a loaded
// library exports flux2_trace_contract_2; the call goes on to the next definition either way.
#include "armo_preload/armo_emit.h"
#include "armo_preload/intercept.h"

#include <stdbool.h>
#include <stdio.h>

#define CONTRACT "flux2_trace_contract_2"

// 0 not checked yet, 1 a loaded flux2 library has the contract, -1 none has. Not asked of the
// caller: a tail call hides it. Nor of the next definition, which can be another armo backend.
static int contract;

static bool agreed(void)
{
  int c = __atomic_load_n(&contract, __ATOMIC_RELAXED);
  if (c != 0) return c > 0;
  c = armo_find(CONTRACT) != NULL ? 1 : -1;
  int unset = 0;
  if (
    __atomic_compare_exchange_n(&contract, &unset, c, false, __ATOMIC_RELAXED, __ATOMIC_RELAXED) &&
    c < 0) {
    fprintf(stderr, "armo_preload: the flux2 library lacks %s; no flux2 events\n", CONTRACT);
  }
  return c > 0;
}

#define FLUX2_POINT(name, event, params, args)                       \
  static void emit_##name params;                                    \
  ARMO_EXPORT void flux2_trace_##name params;                        \
  void flux2_trace_##name params                                     \
  {                                                                  \
    void(*next) params = ARMO_NEXT(flux2_trace_##name, NULL, event); \
    if (agreed()) emit_##name args;                                  \
    if (next != NULL) next args;                                     \
  }                                                                  \
  static void emit_##name params

FLUX2_POINT(
  channel_init, ARMO_EV_FLUX2_CHANNEL_INIT,
  (const void * channel, uint64_t dev, uint64_t ino, const char * segment, int32_t publisher,
   uint32_t slot_count, uint32_t slot_size, int32_t stamp_offset),
  (channel, dev, ino, segment, publisher, slot_count, slot_size, stamp_offset))
{
  armo_ev_flux2_channel_init ev = {
    .channel = ARMO_U64(channel),
    .dev = dev,
    .ino = ino,
    .publisher = publisher,
    .slot_count = slot_count,
    .slot_size = slot_size,
    .stamp_offset = stamp_offset,
  };
  armo_emit_flux2_channel_init(&ev, segment);
}

FLUX2_POINT(
  claim, ARMO_EV_FLUX2_CLAIM, (const void * channel, uint32_t slot, int32_t result, int32_t loan),
  (channel, slot, result, loan))
{
  armo_ev_flux2_claim ev = {
    .channel = ARMO_U64(channel), .slot = slot, .result = result, .loan = loan};
  armo_emit_flux2_claim(&ev);
}

FLUX2_POINT(loan, ARMO_EV_FLUX2_LOAN, (const void * channel, uint32_t slot), (channel, slot))
{
  armo_ev_flux2_loan ev = {.channel = ARMO_U64(channel), .slot = slot};
  armo_emit_flux2_loan(&ev);
}

FLUX2_POINT(
  loan_end, ARMO_EV_FLUX2_LOAN_END, (const void * channel, uint32_t slot, int32_t result),
  (channel, slot, result))
{
  armo_ev_flux2_loan_end ev = {.channel = ARMO_U64(channel), .slot = slot, .result = result};
  armo_emit_flux2_loan_end(&ev);
}

FLUX2_POINT(
  commit, ARMO_EV_FLUX2_COMMIT,
  (const void * channel, uint64_t ticket, uint32_t slot, uint64_t nbytes, int32_t woke,
   uint64_t stamp),
  (channel, ticket, slot, nbytes, woke, stamp))
{
  armo_ev_flux2_commit ev = {
    .channel = ARMO_U64(channel),
    .ticket = ticket,
    .nbytes = nbytes,
    .stamp = stamp,
    .slot = slot,
    .woke = woke,
  };
  armo_emit_flux2_commit(&ev);
}

FLUX2_POINT(
  take, ARMO_EV_FLUX2_TAKE,
  (const void * channel, uint64_t ticket, uint32_t slot, uint64_t lost, uint64_t stamp),
  (channel, ticket, slot, lost, stamp))
{
  armo_ev_flux2_take ev = {
    .channel = ARMO_U64(channel), .ticket = ticket, .lost = lost, .stamp = stamp, .slot = slot};
  armo_emit_flux2_take(&ev);
}

FLUX2_POINT(
  release, ARMO_EV_FLUX2_RELEASE, (const void * channel, uint64_t ticket, int32_t held),
  (channel, ticket, held))
{
  armo_ev_flux2_release ev = {.channel = ARMO_U64(channel), .ticket = ticket, .held = held};
  armo_emit_flux2_release(&ev);
}

FLUX2_POINT(
  refused, ARMO_EV_FLUX2_REFUSED, (const void * channel, int32_t reason), (channel, reason))
{
  armo_ev_flux2_refused ev = {.channel = ARMO_U64(channel), .reason = reason};
  armo_emit_flux2_refused(&ev);
}

FLUX2_POINT(
  wait, ARMO_EV_FLUX2_WAIT, (const void * executor, uint32_t sources, int64_t timeout_ns),
  (executor, sources, timeout_ns))
{
  armo_ev_flux2_wait ev = {
    .executor = ARMO_U64(executor), .timeout_ns = timeout_ns, .sources = sources};
  armo_emit_flux2_wait(&ev);
}

FLUX2_POINT(
  wake, ARMO_EV_FLUX2_WAKE, (const void * executor, int32_t result, uint32_t entry),
  (executor, result, entry))
{
  armo_ev_flux2_wake ev = {.executor = ARMO_U64(executor), .result = result, .entry = entry};
  armo_emit_flux2_wake(&ev);
}

FLUX2_POINT(
  callback_start, ARMO_EV_FLUX2_CALLBACK_START,
  (const void * executor, uint32_t entry, const void * endpoint), (executor, entry, endpoint))
{
  armo_ev_flux2_callback_start ev = {
    .executor = ARMO_U64(executor), .endpoint = ARMO_U64(endpoint), .entry = entry};
  armo_emit_flux2_callback_start(&ev);
}

FLUX2_POINT(
  callback_end, ARMO_EV_FLUX2_CALLBACK_END,
  (const void * executor, uint32_t entry, int32_t delivered, const void * channel,
   const char * segment),
  (executor, entry, delivered, channel, segment))
{
  armo_ev_flux2_callback_end ev = {
    .executor = ARMO_U64(executor),
    .channel = ARMO_U64(channel),
    .entry = entry,
    .delivered = delivered,
  };
  armo_emit_flux2_callback_end(&ev, segment);
}

FLUX2_POINT(
  endpoint_init, ARMO_EV_FLUX2_ENDPOINT_INIT,
  (const void * endpoint, const char * node, const char * key, const char * signpost,
   uint64_t fingerprint, int32_t role, int32_t lang),
  (endpoint, node, key, signpost, fingerprint, role, lang))
{
  armo_ev_flux2_endpoint_init ev = {
    .endpoint = ARMO_U64(endpoint), .fingerprint = fingerprint, .role = role, .lang = lang};
  armo_emit_flux2_endpoint_init(&ev, node, key, signpost);
}

FLUX2_POINT(py_gil, ARMO_EV_FLUX2_PY_GIL, (int32_t phase), (phase))
{
  armo_ev_flux2_py_gil ev = {.phase = phase};
  armo_emit_flux2_py_gil(&ev);
}

FLUX2_POINT(
  py_ready, ARMO_EV_FLUX2_PY_READY, (const void * channel, uint64_t ticket), (channel, ticket))
{
  armo_ev_flux2_py_ready ev = {.channel = ARMO_U64(channel), .ticket = ticket};
  armo_emit_flux2_py_ready(&ev);
}

FLUX2_POINT(
  record_drop, ARMO_EV_FLUX2_RECORD_DROP,
  (const void * channel, uint64_t ticket, uint32_t count, int32_t reason),
  (channel, ticket, count, reason))
{
  armo_ev_flux2_record_drop ev = {
    .channel = ARMO_U64(channel), .ticket = ticket, .count = count, .reason = reason};
  armo_emit_flux2_record_drop(&ev);
}

FLUX2_POINT(gpu_wedged, ARMO_EV_FLUX2_GPU_WEDGED, (int32_t result), (result))
{
  armo_ev_flux2_gpu_wedged ev = {.result = result};
  armo_emit_flux2_gpu_wedged(&ev);
}
