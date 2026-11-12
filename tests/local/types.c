// Integer/float conversions, promotions, bitfields, arithmetic corner cases.
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

struct BF { unsigned a : 3; int b : 5; unsigned c : 12; long d : 40; unsigned e : 1; _Bool f : 1; };
struct BF2 { char x; int a : 7; int b : 9; short s; unsigned u : 31; unsigned v : 2; };
union U { int i; float f; unsigned char b[4]; struct { short lo, hi; }; };
enum E { A, B = 5, C, D = -1, F };
static struct BF gbf = {5, -3, 0xabc, -123456789, 1, 1};

int main(void) {
    signed char sc = -1; unsigned char uc = 255; short sh = -2; unsigned short us = 65535;
    int i = -3; unsigned u = 4000000000u; long l = -5; unsigned long ul = 18446744073709551615ul;
    printf("%d %d %d %d\n", sc, uc, sh, us);
    printf("%d %u %ld %lu\n", sc + uc, u + i, l * u, ul / 3);
    printf("%d %d %d %d\n", sc < uc, i < u, l < u, -1 < 1u);
    printf("%d %d %ld %lu\n", (char)300, (unsigned char)-1, (long)(int)u, (unsigned long)i);
    printf("%d %d %d\n", (short)70000, (int)3000000000u, (int)(unsigned char)-56);
    printf("%d %d %d %d\n", 7 / 2, -7 / 2, 7 % -3, -7 % 3);
    printf("%u %u %d\n", 7u / 2, (unsigned)-7 / 2, (int)((unsigned)-7 % 3));
    printf("%d %d %u %d\n", 1 << 10, -16 >> 2, 0x80000000u >> 31, (int)0x80000000 >> 31);
    printf("%lx %lx %ld\n", 1ul << 63, ~0ul >> 1, (long)1 << 40);
    printf("%d %d %d\n", INT_MAX, INT_MIN, (int)sizeof(long));
    printf("%lu %lu %lu %lu\n", sizeof(char), sizeof(short), sizeof(long long), sizeof(void *));
    printf("%d %d %d\n", !5, !0, ~5);
    printf("%d %d %d %d\n", 5 && 0, 5 || 0, 0 && (1 / 0), 1 || (1 / 0));
    i = 10; i += 5; i -= 3; i *= 2; i /= 5; i %= 3; i <<= 4; i >>= 1; i |= 3; i &= 6; i ^= 15;
    printf("%d\n", i);
    uc = 250; uc += 10; sc = 127; sc++; sh = 32767; sh += 1;
    printf("%d %d %d\n", uc, sc, sh);
    int j = 5; int k = j++ + ++j; printf("%d %d\n", j, k);
    // floats
    float f = 1.5f; double d = 2.25; 
    printf("%g %g %g %g\n", f + d, f * 2, d / f, -d);
    printf("%d %d %u %ld %lu\n", (int)3.99, (int)-3.99, (unsigned)3e9, (long)-1e15, (unsigned long)1.5e19);
    printf("%g %g %g %g\n", (double)i, (float)u, (double)l, (double)ul);
    printf("%g %g\n", (double)(unsigned long)1e19, (float)(unsigned long)(1ul << 63));
    printf("%d %d %d %d %d %d\n", f < d, f > d, f == 1.5, f != 1.5f, d >= 2.25, d <= 2.0);
    printf("%d %d %d\n", !f, !0.0, f && d);
    f += 1; d *= f; d -= 0.125; f /= 2;
    printf("%g %g\n", f, d);
    double nan = 0.0 / 0.0;
    printf("%d %d %d\n", nan == nan, nan != nan, nan < 1.0);
    printf("%.3f %.3e %g %g\n", 1.0 / 3, 12345.678, 1e100, 0x1p-3);
    printf("%g %g\n", 1.0f * INT_MAX, (double)(float)16777217);
    char c2 = f; _Bool b1 = 0.1, b2 = 256, b3 = (_Bool)0; long long ll = 1.9;
    printf("%d %d %d %d %lld\n", c2, b1, b2, b3, ll);
    // bitfields
    struct BF bf = gbf;
    printf("%u %d %u %ld %u %d %lu\n", bf.a, bf.b, bf.c, (long)bf.d, bf.e, bf.f, sizeof bf);
    bf.a = 9; bf.b = 17; bf.c += 0x1000; bf.d = 1L << 39; bf.e ^= 1; bf.f = 5;
    printf("%u %d %u %ld %u %d\n", bf.a, bf.b, bf.c, (long)bf.d, bf.e, bf.f);
    bf.a++; bf.b--; bf.c <<= 2; printf("%u %d %u\n", bf.a, bf.b, bf.c);
    struct BF2 b2s = {1, -5, 200, -7, 0x7fffffff, 3};
    printf("%d %d %d %d %u %u %lu\n", b2s.x, b2s.a, b2s.b, b2s.s, b2s.u, b2s.v, sizeof b2s);
    // union / enum
    union U un; un.f = 1.0f; printf("%x %d %d %d\n", un.i, un.b[3], un.lo, un.hi);
    enum E e = C; printf("%d %d %d %d %d %d\n", A, B, C, D, F, e == 6);
    // 64-bit
    uint64_t x = 0xdeadbeefcafebabeULL; int64_t y = -x;
    printf("%llx %lld %llu\n", (unsigned long long)(x >> 4 ^ x << 7), (long long)(y / 1000), (unsigned long long)(x % 1000000007));
    uint32_t h = 2166136261u; for (const char *p = "hello"; *p; p++) h = (h ^ (unsigned char)*p) * 16777619u;
    printf("%u %d\n", h, (int8_t)h);
    printf("%d %ld %d\n", 'a' + 1, sizeof('a') + sizeof("abc"), "abc"[1]);
    printf("%d %d\n", (int)(sizeof(int[3][4]) / sizeof(int)), (int)sizeof(struct { char c; double d; }));
    return 0;
}
