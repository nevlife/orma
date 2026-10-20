// Marks a stretch inside a callback for orma (design 20.5). Both functions are empty: orma catches
// them by name or attaches a uprobe to them, and without orma a span costs its two calls.
// `name` must have static storage, a string literal: its address pairs a begin with its end.
#ifndef ORMA_SPAN_SPAN_H
#define ORMA_SPAN_SPAN_H

#ifdef __cplusplus
extern "C" {
#endif

void orma_span_begin(const char * name);
void orma_span_end(const char * name);

#ifdef __cplusplus
}

namespace orma
{

class Span
{
public:
  explicit Span(const char * name) : name_(name) { orma_span_begin(name_); }
  ~Span() { orma_span_end(name_); }
  Span(const Span &) = delete;
  Span & operator=(const Span &) = delete;

private:
  const char * name_;
};

}  // namespace orma
#endif

#endif  // ORMA_SPAN_SPAN_H
