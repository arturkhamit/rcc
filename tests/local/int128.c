// 128-bit integers: every operator, the conversions, the calling convention. Compared with gcc.
#include <stdio.h>

typedef __int128 i128;
typedef unsigned __int128 u128;

static void show(const char *what, u128 v) {
  printf("%s: %016llx %016llx\n", what, (unsigned long long)(v >> 64), (unsigned long long)v);
}

// Constants wider than 64 bits in static initializers.
static u128 big = ((u128)0x0123456789abcdefULL << 64) | 0xfedcba9876543210ULL;
static i128 neg = -(i128)5;
static i128 table[3] = {1, -1, (i128)1 << 100};
struct holder { char c; i128 v; long tail; };
static struct holder held = {'x', -((i128)1 << 70), 7};

static u128 mul64(unsigned long a, unsigned long b) { return (u128)a * b; }
static i128 add3(i128 a, i128 b, i128 c) { return a + b + c; }
// The seventh integer word no longer fits in registers: this one goes on the stack, aligned.
static i128 mixed(int a, long b, i128 c, int d, i128 e, char f, i128 g) { return a + b + c + d + e + f + g; }
static struct holder by_value(struct holder h) { h.v = h.v * 2 + h.tail; return h; }

int main(void) {
  show("big", big);
  show("neg", neg);
  show("table", table[0] + table[1] + table[2]);
  show("held", held.v);
  printf("layout: %d %d %d\n", (int)sizeof(i128), (int)_Alignof(i128), (int)sizeof(struct holder));

  // Arithmetic.
  u128 a = big, b = 0xffffffffffffffffULL;
  i128 s = -1234567890123456789LL;
  show("add", a + b);
  show("sub", a - b - b);
  show("mul", a * b);
  show("div", a / (b >> 7));
  show("mod", a % 1000000007);
  show("sdiv", s * 1000000007 / -3);
  show("smod", s * 1000000007 % 97);
  show("neg", -s);
  show("and or xor not", (a & b) | (a ^ ~b));
  show("shl", a << 37);
  show("shr", a >> 69);
  show("sar", (s * 1000000007) >> 5);
  show("mul64", mul64(0xdeadbeefcafebabeULL, 0x123456789abcdef1ULL));

  // Compound assignment, increments.
  u128 x = 1;
  for (int i = 0; i < 100; i++) x += x;
  x--;
  x *= 3;
  x >>= 3;
  x ^= big;
  show("compound", x);
  i128 n = -3;
  n++; ++n; n -= 10; n <<= 80;
  show("signed compound", n);

  // Comparisons, signed and unsigned, and their use as conditions.
  i128 minus = -1, plus = 1;
  u128 huge = (u128)minus;
  printf("compare: %d %d %d %d %d %d %d %d\n", minus < plus, huge < (u128)plus, minus <= minus, plus >= minus, a == big, a != big, a > b, (i128)a > (i128)b);
  int count = 0;
  for (u128 k = (u128)1 << 66; k; k >>= 11) count++;
  u128 zero = 0;
  printf("conditions: %d %d %d %d %d\n", count, !zero, !a, zero || a, a && zero ? 1 : 2);
  show("ternary", zero ? a : b);

  // Conversions.
  long l = -42; unsigned u = 4000000000u; signed char c = -7; double d = 1e30; float f = -2.5e20f;
  show("from long", (i128)l);
  show("from unsigned", u);
  show("from char", (i128)c);
  show("from double", (i128)d);
  show("from float", (i128)f);
  show("unsigned from double", (u128)3e38);
  printf("to: %ld %u %d %g %g %g %d\n", (long)a, (unsigned)a, (signed char)a, (double)a, (double)(float)s, (double)(i128)neg, (_Bool)(a << 127));
  int arr[4] = {10, 20, 30, 40};
  printf("index: %d %d\n", arr[(i128)2], *(arr + (u128)3));

  // Calls: in registers, on the stack, inside a struct, through a pointer.
  show("add3", add3(1, (i128)1 << 64, -3));
  show("mixed", mixed(1, 2, (i128)3 << 64, 4, -5, 6, (i128)7 << 100));
  show("by value", by_value(held).v);
  u128 *p = &big;
  *p += 1;
  p[0] <<= 4;
  show("pointer", *p);
  // A switch compares all 128 bits; the overflow builtins can store into 128 bits.
  for (int i = 0; i < 4; i++) {
    i128 v = i == 0 ? 3 : i == 1 ? -2 : i == 2 ? ((i128)1 << 64) + 3 : -((i128)1 << 64) - 2;
    switch (v) { case 3: puts("switch: 3"); break; case -2: puts("switch: -2"); break; default: puts("switch: other"); }
    switch ((u128)v) { case 3: puts("unsigned switch: 3"); break; case -2: puts("unsigned switch: -2"); break; }
  }
  u128 wide_result; i128 signed_result;
  int o1 = __builtin_mul_overflow((unsigned char)4, -16, &wide_result), o2 = __builtin_sub_overflow(5, 7L, &signed_result);
  show("mul_overflow", wide_result);
  show("sub_overflow", signed_result);
  printf("overflow flags: %d %d\n", o1, o2);
  return 0;
}
