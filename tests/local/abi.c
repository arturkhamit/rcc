// Struct passing/returning, varargs, interop with libc.
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct I1 { char c; };
struct I8 { long a; };
struct I12 { int a, b, c; };
struct I16 { long a, b; };
struct I24 { long a, b, c; };
struct F4 { float f; };
struct F8 { float a, b; };
struct D8 { double d; };
struct D16 { double a, b; };
struct M16 { double d; long l; };
struct M16b { int i; float f; double d; };
struct C3 { char a, b, c; };
struct Big { int a[10]; double d; };

struct I1 r_i1(void) { struct I1 s = {'x'}; return s; }
struct I8 r_i8(long v) { struct I8 s = {v}; return s; }
struct I12 r_i12(int a) { struct I12 s = {a, a + 1, a + 2}; return s; }
struct I16 r_i16(long a) { struct I16 s = {a, -a}; return s; }
struct I24 r_i24(long a) { struct I24 s = {a, a * 2, a * 3}; return s; }
struct F4 r_f4(float f) { struct F4 s = {f}; return s; }
struct F8 r_f8(float f) { struct F8 s = {f, f * 2}; return s; }
struct D8 r_d8(double d) { struct D8 s = {d}; return s; }
struct D16 r_d16(double d) { struct D16 s = {d, d / 2}; return s; }
struct M16 r_m16(double d, long l) { struct M16 s = {d, l}; return s; }
struct M16b r_m16b(int i) { struct M16b s = {i, i + 0.5f, i * 1.25}; return s; }
struct C3 r_c3(void) { struct C3 s = {1, 2, 3}; return s; }
struct Big r_big(int n) { struct Big s; for (int i = 0; i < 10; i++) s.a[i] = n + i; s.d = n * 0.5; return s; }

long take(struct I1 a, struct I8 b, struct I12 c, struct I16 d, struct I24 e, struct F4 f, struct F8 g,
          struct D8 h, struct D16 i, struct M16 j, struct M16b k, struct C3 l, struct Big m, int last) {
    printf("%c %ld %d,%d,%d %ld,%ld %ld,%ld,%ld %g %g,%g %g %g,%g %g,%ld %d,%g,%g %d%d%d %d..%d,%g %d\n",
           a.c, b.a, c.a, c.b, c.c, d.a, d.b, e.a, e.b, e.c, f.f, g.a, g.b, h.d, i.a, i.b, j.d, j.l,
           k.i, k.f, k.d, l.a, l.b, l.c, m.a[0], m.a[9], m.d, last);
    return b.a + d.b + e.c;
}

int many(int a, int b, int c, int d, int e, int f, int g, int h, double x1, double x2, double x3, double x4,
         double x5, double x6, double x7, double x8, double x9, char i, short j, long k) {
    printf("%d %d %d %d %d %d %d %d %g %g %g %g %g %g %g %g %g %d %d %ld\n", a, b, c, d, e, f, g, h, x1, x2, x3, x4,
           x5, x6, x7, x8, x9, i, j, k);
    return a + h + (int)x9 + i + j;
}

int sum_ints(int n, ...) {
    va_list ap;
    va_start(ap, n);
    int s = 0;
    for (int i = 0; i < n; i++) s += va_arg(ap, int);
    va_end(ap);
    return s;
}

double mixed(const char *fmt, ...) {
    va_list ap, ap2;
    va_start(ap, fmt);
    va_copy(ap2, ap);
    double s = 0;
    for (const char *p = fmt; *p; p++) {
        switch (*p) {
        case 'i': s += va_arg(ap, int); break;
        case 'l': s += va_arg(ap, long); break;
        case 'd': s += va_arg(ap, double); break;
        case 's': s += strlen(va_arg(ap, char *)); break;
        case 'S': { struct I16 v = va_arg(ap, struct I16); s += v.a + v.b; break; }
        case 'D': { struct D16 v = va_arg(ap, struct D16); s += v.a + v.b; break; }
        case 'M': { struct M16 v = va_arg(ap, struct M16); s += v.d + v.l; break; }
        case 'B': { struct I24 v = va_arg(ap, struct I24); s += v.a + v.b + v.c; break; }
        case 'F': { struct F8 v = va_arg(ap, struct F8); s += v.a + v.b; break; }
        }
    }
    if (fmt[0] == 'i') s += va_arg(ap2, int) * 1000;
    va_end(ap);
    va_end(ap2);
    return s;
}

void logf_(const char *fmt, ...) {
    char buf[128];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    puts(buf);
}

int cmp_int(const void *a, const void *b) { return *(const int *)a - *(const int *)b; }
typedef int (*binop)(int, int);
int add(int a, int b) { return a + b; }
int mul(int a, int b) { return a * b; }
binop pick(int i) { return i ? mul : add; }
long (*fpp(void))(struct I16) { static long (*p)(struct I16); return p; }

int main(void) {
    struct I16 d = r_i16(7);
    struct Big m = r_big(100);
    long r = take(r_i1(), r_i8(42), r_i12(5), d, r_i24(3), r_f4(1.5f), r_f8(2.25f), r_d8(3.5), r_d16(9.0),
                  r_m16(1.25, 77), r_m16b(4), r_c3(), m, 99);
    printf("%ld\n", r);
    printf("%d\n", many(1, 2, 3, 4, 5, 6, 7, 8, .1, .2, .3, .4, .5, .6, .7, .8, 9.9, 'c', -3, 1L << 40));
    printf("%d %d\n", sum_ints(3, 1, 2, 3), sum_ints(10, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10));
    struct D16 dd = {1.5, 2.5};
    struct M16 mm = {0.5, 10};
    struct I24 bb = {1, 2, 3};
    struct F8 ff = {0.25f, 0.5f};
    printf("%g\n", mixed("ildsSDMBFidddddddddSD", 1, 2L, 3.5, "four", d, dd, mm, bb, ff, 5, 1., 2., 3., 4., 5., 6., 7., 8., 9., d, dd));
    logf_("%s=%d %5.2f %c %lu %lld %x", "k", -5, 3.14159, 'z', 123456789012UL, -9LL, 255);
    int arr[] = {5, 3, 9, 1, 7};
    qsort(arr, 5, sizeof(int), cmp_int);
    for (int i = 0; i < 5; i++) printf("%d ", arr[i]);
    div_t q = div(17, 5);
    ldiv_t lq = ldiv(-17L, 5L);
    printf("\n%d %d %ld %ld\n", q.quot, q.rem, lq.quot, lq.rem);
    printf("%d %d %d\n", pick(0)(3, 4), pick(1)(3, 4), (*pick(1))(5, 6));
    printf("%d\n", fpp() == 0);
    char buf[64];
    sprintf(buf, "%s-%d-%.3f", "a", 1, 2.0);
    printf("%s %zu %d\n", buf, strlen(buf), atoi("123") + (int)strtol("ff", 0, 16));
    return 0;
}
