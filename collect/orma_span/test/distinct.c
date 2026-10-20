// The two markers are two functions at two addresses in liborma_span.so: a uprobe on one must not
// fire for the other.
#define _GNU_SOURCE
#include "orma_span/span.h"

#include <dlfcn.h>
#include <stdio.h>
#include <string.h>

int main(void)
{
  void * begin = dlsym(RTLD_DEFAULT, "orma_span_begin");
  void * end = dlsym(RTLD_DEFAULT, "orma_span_end");
  Dl_info b, e;
  if (
    begin == NULL || end == NULL || begin == end || dladdr(begin, &b) == 0 ||
    dladdr(end, &e) == 0 || strcmp(b.dli_sname, "orma_span_begin") != 0 ||
    strcmp(e.dli_sname, "orma_span_end") != 0 || strstr(b.dli_fname, "liborma_span") == NULL) {
    fprintf(stderr, "orma_span_begin %p, orma_span_end %p\n", begin, end);
    return 1;
  }
  orma_span_begin("distinct");
  orma_span_end("distinct");
  printf("ok\n");
  return 0;
}
