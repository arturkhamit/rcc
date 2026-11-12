#include <stdio.h>
#include <stdarg.h>
#include <stdlib.h>
#include <float.h>
#include <math.h>

struct LD { long double v; };
struct Mix { char c; long double v; int i; };

long double g1 = 1.5L, g2 = -0.1L, g3 = 1e4000L, g4 = 3;
long double garr[3] = { 1.25L, 2.5, -7 };
struct Mix gm = { 'x', 0.3L, 42 };

long double add(long double a, long double b) { return a + b; }
long double mix(int a, long double b, double c, long double d, float e) { return a + b * c - d / e; }
struct LD wrap(long double x) { struct LD r = { x * 2 }; return r; }
long double unwrap(struct LD s) { return s.v; }
long double many(long a, long b, long c, long d, long e, long f, long g, long double x, long h, long double y) { return a + b + c + d + e + f + g + x + h + y; }

long double vsum(int n, ...) {
  va_list ap;
  long double s = 0;
  va_start(ap, n);
  for (int i = 0; i < n; i++) {
    if (i % 3 == 0) s += va_arg(ap, long double);
    else if (i % 3 == 1) s += va_arg(ap, int);
    else s += va_arg(ap, double);
  }
  va_end(ap);
  return s;
}

int main(void) {
  long double a = 1.0L / 3.0L, b = 2, c;
  printf("%zu %zu %zu\n", sizeof(long double), _Alignof(long double), sizeof(struct Mix));
  printf("%.20Lf %.20Lf\n", a, b);
  printf("%Lg %Lg %Lg %Lg\n", g1, g2, g3, g4);
  printf("%Lg %Lg %Lg\n", garr[0], garr[1], garr[2]);
  printf("%c %.19Lf %d\n", gm.c, gm.v, gm.i);
  c = a * b + 1; printf("%.20Lf\n", c);
  c -= 0.25L; c *= 3; c /= 7; printf("%.20Lf\n", c);
  c++; ++c; c--; printf("%.20Lf %d\n", c, c > 1.0L);
  printf("%d %d %d %d %d %d\n", a < b, a <= b, a > b, a >= b, a == b, a != b);
  printf("%.20Lf\n", add(a, b));
  printf("%.20Lf\n", mix(3, a, 2.5, b, 4.0f));
  printf("%.20Lf %.20Lf\n", wrap(a).v, unwrap(wrap(1.125L)));
  printf("%.20Lf\n", many(1, 2, 3, 4, 5, 6, 7, 0.5L, 8, 0.25L));
  printf("%.20Lf\n", vsum(6, 1.5L, 2, 3.25, a, -4, 0.5));
  /* conversions */
  int i = -7; unsigned u = 4000000000u; long l = -1234567890123L; unsigned long ul = 18446744073709551615UL;
  float f = 1.5f; double d = -2.25; char ch = -3; unsigned short us = 65535;
  long double x1 = i, x2 = u, x3 = l, x4 = ul, x5 = f, x6 = d, x7 = ch, x8 = us;
  printf("%Lf %Lf %Lf %Lf %Lf %Lf %Lf %Lf\n", x1, x2, x3, x4, x5, x6, x7, x8);
  long double big = 1.8e19L, neg = -123.75L, small = 300.9L;
  printf("%d %u %ld %lu %lu %f %f\n", (int)neg, (unsigned)small, (long)neg, (unsigned long)big, (unsigned long)small, (double)neg, (float)small);
  printf("%d %d %d\n", (char)small, (unsigned char)small, (short)neg);
  printf("%d %d %d %d\n", !a, !!a, a && b, (_Bool)0.0L);
  long double z = 0;
  if (z) printf("bad\n"); else printf("zero\n");
  printf("%Lg\n", z ? a : b);
  printf("%Lg %Lg\n", -a, +a);
  printf("%Lg %Lg %Lg\n", LDBL_MAX, LDBL_MIN, LDBL_EPSILON);
  printf("%d %d\n", LDBL_MANT_DIG, LDBL_DIG);
  printf("%Lg %Lg %Lg\n", sqrtl(2.0L), powl(2.0L, 0.5L), fabsl(-3.5L));
  printf("%Lg %Lg\n", strtold("1.25e3", 0), 0x1.8p1L);
  long double arr[4] = { 1, 2, 3, 4 }, *p = arr, s = 0;
  for (int k = 0; k < 4; k++) s += *p++ * (k + 1);
  printf("%Lg\n", s);
  struct Mix m = { 'q', 9.5L, 7 }, m2 = m;
  m2.v += m.i;
  printf("%c %Lg %d\n", m2.c, m2.v, m2.i);
  /* many discarded values must not overflow the x87 stack */
  for (int k = 0; k < 100; k++) { a + b; add(a, b); (void)(a * b); a, b; }
  for (long double q = 0; q < 3; q++) s += q;
  printf("%Lg %Lg\n", s, ({ long double t = s; t * 2; }));
  double nan = 0.0; nan = nan / nan;
  printf("%d %d %d\n", nan ? 1 : 0, !nan, nan != nan);
  return 0;
}
