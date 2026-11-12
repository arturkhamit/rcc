/* Processed before every translation unit: GNU builtins that can be written in C.
   The bit-counting ones call libgcc, which the linker driver always links. */
int __clzdi2(unsigned long), __ctzdi2(unsigned long), __popcountdi2(unsigned long), __paritydi2(unsigned long);
int __clrsbdi2(long), __signbit(double), __signbitf(float);
unsigned int __bswapsi2(unsigned int);
unsigned long __bswapdi2(unsigned long);

#define __builtin_clz(x) (__clzdi2((unsigned)(x)) - 32)
#define __builtin_clzl(x) __clzdi2(x)
#define __builtin_clzll(x) __clzdi2(x)
#define __builtin_ctz(x) __ctzdi2((unsigned)(x))
#define __builtin_ctzl(x) __ctzdi2(x)
#define __builtin_ctzll(x) __ctzdi2(x)
#define __builtin_popcount(x) __popcountdi2((unsigned)(x))
#define __builtin_popcountl(x) __popcountdi2(x)
#define __builtin_popcountll(x) __popcountdi2(x)
#define __builtin_parity(x) __paritydi2((unsigned)(x))
#define __builtin_parityl(x) __paritydi2(x)
#define __builtin_parityll(x) __paritydi2(x)
#define __builtin_clrsb(x) (__clrsbdi2((int)(x)) - 32)
#define __builtin_clrsbl(x) __clrsbdi2(x)
#define __builtin_clrsbll(x) __clrsbdi2(x)
#define __builtin_bswap16(x) ((unsigned short)(__bswapsi2(x) >> 16))
#define __builtin_bswap32(x) __bswapsi2(x)
#define __builtin_bswap64(x) __bswapdi2(x)

#define __builtin_isnan(x) ((x) != (x))
#define __builtin_isinf(x) ((x) == (x) && (x) - (x) != 0)
#define __builtin_isfinite(x) ((x) - (x) == 0)
#define __builtin_signbit(x) __signbit(x)
#define __builtin_signbitf(x) __signbitf(x)
#define __builtin_isgreater(a, b) ((a) > (b))
#define __builtin_isgreaterequal(a, b) ((a) >= (b))
#define __builtin_isless(a, b) ((a) < (b))
#define __builtin_islessequal(a, b) ((a) <= (b))
#define __builtin_islessgreater(a, b) ((a) < (b) || (a) > (b))
#define __builtin_isunordered(a, b) ((a) != (a) || (b) != (b))
#define __builtin_inf() (1.0 / 0.0)
#define __builtin_inff() (1.0f / 0.0f)
#define __builtin_infl() (1.0L / 0.0L)
#define __builtin_huge_val() (1.0 / 0.0)
#define __builtin_huge_valf() (1.0f / 0.0f)
#define __builtin_huge_vall() (1.0L / 0.0L)
#define __builtin_nan(s) (-(0.0 / 0.0))
#define __builtin_nanf(s) (-(0.0f / 0.0f))
#define __builtin_nanl(s) (-(0.0L / 0.0L))
#define __builtin_expect_with_probability(e, v, p) (e)
#define __builtin_assume_aligned(p, ...) (p)

/* Overflow-checked arithmetic: the operands as sign + magnitude, the result
   checked against the width and signedness of *r. */
static int __rcc_ovf(int op, unsigned long a, int na, unsigned long b, int nb, int bits, int sgn, unsigned long *out) {
  unsigned long m, max;
  int n, carry = 0;
  if (op == 1) nb = !nb;
  if (op == 2) { m = a * b; n = na ^ nb; carry = a && m / a != b; }
  else if (na == nb) { m = a + b; n = na; carry = m < a; }
  else if (a >= b) { m = a - b; n = na; }
  else { m = b - a; n = nb; }
  *out = n ? -m : m;
  if (carry) return 1;
  if (!m) return 0;
  max = bits == 64 && !sgn ? -1UL : (1UL << (bits - sgn)) - 1;
  if (sgn) return n ? m > max + 1 : m > max;
  return n ? 1 : m > max;
}
#define __rcc_ovf_m(op, a, b, r) ({ __typeof__(a) __a = (a); __typeof__(b) __b = (b); __typeof__(r) __r = (r); unsigned long __o; \
  int __f = __rcc_ovf(op, __a < 0 ? -(unsigned long)__a : (unsigned long)__a, __a < 0, __b < 0 ? -(unsigned long)__b : (unsigned long)__b, __b < 0, \
                      (__typeof__(*__r))2 == 1 ? 1 : (int)sizeof(*__r) * 8, (__typeof__(*__r))-1 < 0, &__o); \
  *__r = (__typeof__(*__r))__o; __f; })
#define __builtin_add_overflow(a, b, r) __rcc_ovf_m(0, a, b, r)
#define __builtin_sub_overflow(a, b, r) __rcc_ovf_m(1, a, b, r)
#define __builtin_mul_overflow(a, b, r) __rcc_ovf_m(2, a, b, r)
#define __builtin_add_overflow_p(a, b, c) ({ __typeof__(c) __t; __builtin_add_overflow(a, b, &__t); })
#define __builtin_sub_overflow_p(a, b, c) ({ __typeof__(c) __t; __builtin_sub_overflow(a, b, &__t); })
#define __builtin_mul_overflow_p(a, b, c) ({ __typeof__(c) __t; __builtin_mul_overflow(a, b, &__t); })
#define __builtin_sadd_overflow(a, b, r) __builtin_add_overflow((int)(a), (int)(b), (int *)(r))
#define __builtin_saddl_overflow(a, b, r) __builtin_add_overflow((long)(a), (long)(b), (long *)(r))
#define __builtin_saddll_overflow(a, b, r) __builtin_add_overflow((long long)(a), (long long)(b), (long long *)(r))
#define __builtin_uadd_overflow(a, b, r) __builtin_add_overflow((unsigned)(a), (unsigned)(b), (unsigned *)(r))
#define __builtin_uaddl_overflow(a, b, r) __builtin_add_overflow((unsigned long)(a), (unsigned long)(b), (unsigned long *)(r))
#define __builtin_uaddll_overflow(a, b, r) __builtin_add_overflow((unsigned long long)(a), (unsigned long long)(b), (unsigned long long *)(r))
#define __builtin_ssub_overflow(a, b, r) __builtin_sub_overflow((int)(a), (int)(b), (int *)(r))
#define __builtin_ssubl_overflow(a, b, r) __builtin_sub_overflow((long)(a), (long)(b), (long *)(r))
#define __builtin_ssubll_overflow(a, b, r) __builtin_sub_overflow((long long)(a), (long long)(b), (long long *)(r))
#define __builtin_usub_overflow(a, b, r) __builtin_sub_overflow((unsigned)(a), (unsigned)(b), (unsigned *)(r))
#define __builtin_usubl_overflow(a, b, r) __builtin_sub_overflow((unsigned long)(a), (unsigned long)(b), (unsigned long *)(r))
#define __builtin_usubll_overflow(a, b, r) __builtin_sub_overflow((unsigned long long)(a), (unsigned long long)(b), (unsigned long long *)(r))
#define __builtin_smul_overflow(a, b, r) __builtin_mul_overflow((int)(a), (int)(b), (int *)(r))
#define __builtin_smull_overflow(a, b, r) __builtin_mul_overflow((long)(a), (long)(b), (long *)(r))
#define __builtin_smulll_overflow(a, b, r) __builtin_mul_overflow((long long)(a), (long long)(b), (long long *)(r))
#define __builtin_umul_overflow(a, b, r) __builtin_mul_overflow((unsigned)(a), (unsigned)(b), (unsigned *)(r))
#define __builtin_umull_overflow(a, b, r) __builtin_mul_overflow((unsigned long)(a), (unsigned long)(b), (unsigned long *)(r))
#define __builtin_umulll_overflow(a, b, r) __builtin_mul_overflow((unsigned long long)(a), (unsigned long long)(b), (unsigned long long *)(r))
