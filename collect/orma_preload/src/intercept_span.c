// liborma_span.so's markers (design 20.5), always caught. The name's address is the key that pairs
// a begin with its end; only the begin carries the name.
#include "orma_preload/intercept.h"
#include "orma_preload/orma_emit.h"

#define SPAN "liborma_span.so"

ORMA_EXPORT void orma_span_begin(const char * name);
ORMA_EXPORT void orma_span_end(const char * name);

void orma_span_begin(const char * name)
{
  orma_ev_span ev = {.key = ORMA_U64(name)};
  orma_emit_span_begin(&ev, name);
  void (*next)(const char *) = ORMA_NEXT(orma_span_begin, SPAN, ORMA_EV_SPAN_BEGIN);
  if (next != NULL) {
    next(name);
  }
}

void orma_span_end(const char * name)
{
  orma_ev_span ev = {.key = ORMA_U64(name)};
  orma_emit_span_end(&ev);
  void (*next)(const char *) = ORMA_NEXT(orma_span_end, SPAN, ORMA_EV_SPAN_END);
  if (next != NULL) {
    next(name);
  }
}
