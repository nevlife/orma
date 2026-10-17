// Marks a stretch inside a callback for armo (design 20.5). Both functions are empty: armo catches
// them by name or attaches a uprobe to them, and without armo a span costs its two calls.
// `name` must have static storage, a string literal: its address pairs a begin with its end.
#ifndef ARMO_SPAN_SPAN_H
#define ARMO_SPAN_SPAN_H

#ifdef __cplusplus
extern "C" {
#endif

void armo_span_begin(const char * name);
void armo_span_end(const char * name);

#ifdef __cplusplus
}

namespace armo
{

class Span
{
public:
  explicit Span(const char * name) : name_(name) { armo_span_begin(name_); }
  ~Span() { armo_span_end(name_); }
  Span(const Span &) = delete;
  Span & operator=(const Span &) = delete;

private:
  const char * name_;
};

}  // namespace armo
#endif

#endif  // ARMO_SPAN_SPAN_H
