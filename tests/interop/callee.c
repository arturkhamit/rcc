#include "defs.h"
struct S1 f_s1(struct S1 s, int k) { s.c += k; return s; }
struct S3 f_s3(struct S3 s, int k) { s.c[0] += k; s.c[2] -= k; return s; }
struct S8 f_s8(struct S8 s, int k) { s.a += k; s.f *= k; return s; }
struct S12 f_s12(int k, struct S12 s) { s.a += k; s.c -= k; return s; }
struct S16 f_s16(struct S16 s, double k) { s.a *= 2; s.d += k; return s; }
struct SFF f_sff(float k, struct SFF s) { s.a += k; s.b -= k; return s; }
struct SDD f_sdd(struct SDD s, struct SDD t) { s.a += t.b; s.b += t.a; return s; }
struct SFI f_sfi(struct SFI s, int k) { s.f += k; s.i -= k; s.d *= k; return s; }
struct S24 f_s24(struct S24 s, long k) { s.a += k; s.b -= k; s.c *= k; return s; }
struct SLD f_sld(struct SLD s, long double k) { s.v = s.v * k + 1; return s; }
struct SMIX f_smix(int k, struct SMIX s) { s.c += k; s.v /= k; s.i *= k; return s; }
struct P9 f_p9(struct P9 s, int k) { s.c += k; s.l -= k; return s; }
struct P5 f_p5(int k, struct P5 s) { s.c -= k; s.i += k; return s; }
struct BF f_bf(struct BF s, int k) { s.a += k; s.b -= k; s.c += (unsigned long)k << 33; return s; }
union U f_u(union U u, int k) { u.c[8] = k; u.i += k; return u; }
struct NEST f_nest(struct NEST n, struct ARR a) { n.s.a += a.c[4]; n.s.f += a.f[1]; n.f.a += a.f[0]; n.f.b -= a.c[0]; return n; }
struct ARR f_arr(struct ARR a, int k) { a.f[0] += k; a.f[1] -= k; a.c[4] = k; return a; }
double f_many(int a, double b, long c, float d, char e, double f, short g, double h, long i, double j, int k, double l, long m, double n, int o, double p, float q, double r, long double s, struct S16 t, struct SFF u, int v) {
  return a + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7 + h * 8 + i * 9 + j * 10 + k * 11 + l * 12 + m * 13 + n * 14 + o * 15 + p * 16 + q * 17 + r * 18 + s * 19 + t.a * 20 + t.d * 21 + u.a * 22 + u.b * 23 + v * 24;
}
long double f_va(const char *fmt, ...) {
  va_list ap; long double r = 0; va_start(ap, fmt);
  for (; *fmt; fmt++) {
    switch (*fmt) {
    case 'i': r += va_arg(ap, int); break;
    case 'l': r += va_arg(ap, long); break;
    case 'd': r += va_arg(ap, double); break;
    case 'L': r += va_arg(ap, long double); break;
    case 's': { struct S16 s = va_arg(ap, struct S16); r += s.a + s.d; break; }
    case 'f': { struct SFF s = va_arg(ap, struct SFF); r += s.a + s.b; break; }
    case 'b': { struct S24 s = va_arg(ap, struct S24); r += s.a + s.b + s.c; break; }
    case 'p': { struct P9 s = va_arg(ap, struct P9); r += s.c + s.l; break; }
    }
  }
  va_end(ap); return r;
}
long f_fp(long (*cb)(struct S16, int), struct S16 s) { return cb(s, 5) + cb_s16(s, 1); }
double f_knr(f, c, s, d) float f; char c; short s; double d; { return f * 2 + c + s + d + sizeof f + sizeof c; }
