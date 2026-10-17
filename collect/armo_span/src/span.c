// Two addresses, one per uprobe: interposable exports are neither inlined nor folded, and the
// nop keeps a linker's identical code folding off them too.
#include "armo_span/span.h"

#define SPAN_EXPORT __attribute__((visibility("default"), noinline))

SPAN_EXPORT void armo_span_begin(const char * name)
{
  __asm__ volatile("" : : "r"(name) : "memory");
}

SPAN_EXPORT void armo_span_end(const char * name)
{
  __asm__ volatile("nop" : : "r"(name) : "memory");
}
