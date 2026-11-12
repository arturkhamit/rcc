#include <stdio.h>
#include <limits.h>
struct S { int a; };
#define tn(x) _Generic((x), int: "int", long: "long", double: "double", float: "float", char *: "char*", struct S: "S", default: "other")
int tbl[10] = { [1 ... 3] = 7, [5] = 1, [8 ... 9] = 2 };
int classify(int c) {
  switch (c) { case 'a' ... 'z': return 1; case '0' ... '9': return 2; case -5 ... -1: return 3; default: return 0; }
}
int interp(const char *prog) {
  static void *ops[] = { &&inc, &&dec, &&dbl, &&halt };
  int acc = 0;
  goto *ops[*prog++ - '0'];
inc: acc++; goto *ops[*prog++ - '0'];
dec: acc--; goto *ops[*prog++ - '0'];
dbl: acc *= 2; goto *ops[*prog++ - '0'];
halt: return acc;
}
#define SWAPCHK(x) ({ __label__ out; int r = 0; if (x) goto out; r = 1; out: r; })
_Static_assert(sizeof(int) == 4, "int");
int main(void) {
  struct S s; char *p = 0; float f = 0;
  printf("%s %s %s %s %s %s %s\n", tn(1), tn(1L), tn(1.0), tn(f), tn(p), tn(s), tn(&s));
  for (int i = 0; i < 10; i++) printf("%d ", tbl[i]);
  printf("\n%d %d %d %d\n", classify('q'), classify('5'), classify(-3), classify('!'));
  printf("%d\n", interp("0002213"));
  printf("%d %d\n", SWAPCHK(1), SWAPCHK(0));
  printf("%d %d %d %d\n", __builtin_clz(1), __builtin_ctz(8), __builtin_popcount(255), __builtin_parity(7));
  printf("%d %d %d\n", __builtin_clzll(1ULL), __builtin_ctzl(1UL << 40), __builtin_clrsb(-1));
  printf("%x %x %lx\n", __builtin_bswap16(0x1234), __builtin_bswap32(0x12345678), __builtin_bswap64(0x0102030405060708UL));
  int r; long lr; unsigned char uc; unsigned long ul; _Bool b; short sh;
#define T(e, v) do { int f_ = (e); printf("%d %ld\n", f_, (long)(v)); } while (0)
  T(__builtin_add_overflow(INT_MAX, 1, &r), r);
  T(__builtin_add_overflow(INT_MAX - 1, 1, &r), r);
  T(__builtin_mul_overflow(LONG_MAX, 2L, &lr), lr);
  T(__builtin_mul_overflow(-3L, 5, &lr), lr);
  T(__builtin_sub_overflow(5, 10, &uc), uc);
  T(__builtin_add_overflow(200, 55, &uc), uc);
  T(__builtin_sub_overflow(0UL, 1, &ul), ul);
  T(__builtin_ssubl_overflow(LONG_MIN, 1, &lr), lr);
  T(__builtin_add_overflow(LONG_MIN, -1L, &lr), lr);
  T(__builtin_mul_overflow(LONG_MIN, -1L, &lr), lr);
  T(__builtin_mul_overflow(1L << 31, 1L << 31, &ul), ul);
  T(__builtin_mul_overflow(1UL << 32, 1UL << 32, &ul), ul);
  T(__builtin_mul_overflow(-200, 200, &sh), sh);
  T(__builtin_sub_overflow(-32768, 0, &sh), sh);
  T(__builtin_umul_overflow(65536u, 65536u, (unsigned *)&r), r);
  T(__builtin_add_overflow_p(INT_MAX, 1, (int)0), 0);
  T(__builtin_add_overflow_p(INT_MAX, 1, (long)0), 0);
  printf("%d %d\n", __builtin_types_compatible_p(int, typeof(r)), __builtin_types_compatible_p(int, long));
  printf("%d %d %d\n", __builtin_constant_p(3 + 4), __builtin_constant_p(r), __builtin_choose_expr(1, 10, 20));
  printf("%d %d\n", __builtin_classify_type(1), __builtin_classify_type(1.0));
  printf("%d\n", __builtin_frame_address(0) != 0 && __builtin_return_address(0) != 0);
  asm volatile ("" : : "r" (r) : "memory");
  return 0;
}
