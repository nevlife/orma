// libarmo_span.so's markers (design 20.5), always caught. The name's address is the key that pairs
// a begin with its end; only the begin carries the name.
#include "armo_preload/armo_emit.h"
#include "armo_preload/intercept.h"

#define SPAN "libarmo_span.so"

ARMO_EXPORT void armo_span_begin(const char * name);
ARMO_EXPORT void armo_span_end(const char * name);

void armo_span_begin(const char * name)
{
  armo_ev_span ev = {.key = ARMO_U64(name)};
  armo_emit_span_begin(&ev, name);
  void (*next)(const char *) = ARMO_NEXT(armo_span_begin, SPAN, ARMO_EV_SPAN_BEGIN);
  if (next != NULL) next(name);
}

void armo_span_end(const char * name)
{
  armo_ev_span ev = {.key = ARMO_U64(name)};
  armo_emit_span_end(&ev);
  void (*next)(const char *) = ARMO_NEXT(armo_span_end, SPAN, ARMO_EV_SPAN_END);
  if (next != NULL) next(name);
}
