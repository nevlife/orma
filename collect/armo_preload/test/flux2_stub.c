// Stands in for libflux2_ffi while it changes: the flux2_trace_* of contract 2 with the signatures
// of design 11.2, counting the calls that reach them, and stub_run, which calls them through this
// library's PLT as libflux2_ffi does. Built with and without flux2_trace_contract_2.
#include <stddef.h>
#include <stdint.h>

#define TRACE __attribute__((visibility("default"), noinline))

// Calls that reached each function, so that no two bodies are the same and get merged.
static uint64_t counts[16];

#define BODY(tag) __atomic_fetch_add(&counts[tag], 1, __ATOMIC_RELAXED)

TRACE uint64_t stub_calls(void)
{
  uint64_t n = 0;
  for (int i = 0; i < 16; i++) n += counts[i];
  return n;
}

#ifdef STUB_CONTRACT
TRACE void flux2_trace_contract_2(void)
{
  __asm__ volatile("");
}
#endif

TRACE void flux2_trace_channel_init(
  const void * channel, uint64_t dev, uint64_t ino, const char * segment, int32_t publisher,
  uint32_t slot_count, uint32_t slot_size, int32_t stamp_offset)
{
  (void)channel, (void)dev, (void)ino, (void)segment, (void)publisher, (void)slot_count,
    (void)slot_size, (void)stamp_offset;
  BODY(0);
}

TRACE void flux2_trace_claim(const void * channel, uint32_t slot, int32_t result, int32_t loan)
{
  (void)channel, (void)slot, (void)result, (void)loan;
  BODY(1);
}

TRACE void flux2_trace_loan(const void * channel, uint32_t slot)
{
  (void)channel, (void)slot;
  BODY(2);
}

TRACE void flux2_trace_loan_end(const void * channel, uint32_t slot, int32_t result)
{
  (void)channel, (void)slot, (void)result;
  BODY(3);
}

TRACE void flux2_trace_commit(
  const void * channel, uint64_t ticket, uint32_t slot, uint64_t nbytes, int32_t woke,
  uint64_t stamp)
{
  (void)channel, (void)ticket, (void)slot, (void)nbytes, (void)woke, (void)stamp;
  BODY(4);
}

TRACE void flux2_trace_take(
  const void * channel, uint64_t ticket, uint32_t slot, uint64_t lost, uint64_t stamp)
{
  (void)channel, (void)ticket, (void)slot, (void)lost, (void)stamp;
  BODY(5);
}

TRACE void flux2_trace_release(const void * channel, uint64_t ticket, int32_t held)
{
  (void)channel, (void)ticket, (void)held;
  BODY(6);
}

TRACE void flux2_trace_refused(const void * channel, int32_t reason)
{
  (void)channel, (void)reason;
  BODY(7);
}

TRACE void flux2_trace_wait(const void * executor, uint32_t sources, int64_t timeout_ns)
{
  (void)executor, (void)sources, (void)timeout_ns;
  BODY(8);
}

TRACE void flux2_trace_wake(const void * executor, int32_t result, uint32_t entry)
{
  (void)executor, (void)result, (void)entry;
  BODY(9);
}

TRACE void flux2_trace_callback_start(const void * executor, uint32_t entry, const void * endpoint)
{
  (void)executor, (void)entry, (void)endpoint;
  BODY(10);
}

TRACE void flux2_trace_callback_end(
  const void * executor, uint32_t entry, int32_t delivered, const void * channel,
  const char * segment)
{
  (void)executor, (void)entry, (void)delivered, (void)channel, (void)segment;
  BODY(11);
}

TRACE void flux2_trace_endpoint_init(
  const void * endpoint, const char * node, const char * key, const char * signpost,
  uint64_t fingerprint, int32_t role, int32_t lang)
{
  (void)endpoint, (void)node, (void)key, (void)signpost, (void)fingerprint, (void)role, (void)lang;
  BODY(12);
}

TRACE void flux2_trace_py_gil(int32_t phase)
{
  (void)phase;
  BODY(13);
}

TRACE void flux2_trace_py_ready(const void * channel, uint64_t ticket)
{
  (void)channel, (void)ticket;
  BODY(14);
}

TRACE void flux2_trace_record_drop(
  const void * channel, uint64_t ticket, uint32_t count, int32_t reason)
{
  (void)channel, (void)ticket, (void)count, (void)reason;
  BODY(15);
}

#define CH ((const void *)0xC0)
#define EX ((const void *)0xE0)
#define EX2 ((const void *)0xE2)
#define EP ((const void *)0xEE)

// First a pass of EX on entry 1 that delivers nothing. Each round: a wait and wake of EX, then its
// callback on entry 1, which publishes and runs EX2's callback on entry 3 nested inside it. The
// other functions once. 10 + 10 * rounds calls.
TRACE void stub_run(int rounds)
{
  flux2_trace_channel_init(CH, 7, 8, "/flux2.seg", 1, 4, 4096, 0);
  flux2_trace_endpoint_init(EP, "/armo_test/node", "/key", "/flux2.v", 0xF00D, 1, 0);
  flux2_trace_callback_start(EX, 1, EP);
  flux2_trace_callback_end(EX, 1, 0, CH, "/flux2.seg");
  for (int r = 1; r <= rounds; r++) {
    flux2_trace_wait(EX, 2, -1);
    flux2_trace_wake(EX, 1, 1);
    flux2_trace_callback_start(EX, 1, EP);
    flux2_trace_claim(CH, 0, 0, 0);
    flux2_trace_commit(CH, (uint64_t)r, 0, 64, 1, 0x5000000000u + (uint64_t)r);
    flux2_trace_callback_start(EX2, 3, NULL);
    flux2_trace_take(CH, (uint64_t)r, 0, 0, 0x5000000000u + (uint64_t)r);
    flux2_trace_release(CH, (uint64_t)r, 0);
    flux2_trace_callback_end(EX2, 3, 1, CH, "/flux2.seg");
    flux2_trace_callback_end(EX, 1, 1, CH, "/flux2.seg");
  }
  flux2_trace_loan(CH, 1);
  flux2_trace_loan_end(CH, 1, 0);
  flux2_trace_refused(CH, 2);
  flux2_trace_py_gil(0);
  flux2_trace_py_ready(CH, 9);
  flux2_trace_record_drop(CH, 0, 3, 0);
}
