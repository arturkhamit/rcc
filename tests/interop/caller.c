#include <stdio.h>
#include "defs.h"
long cb_s16(struct S16 s, int k) { return s.a * k + (long)s.d; }
int main(void) {
  struct S1 s1 = f_s1((struct S1){ 5 }, 3); printf("%d\n", s1.c);
  struct S3 s3 = f_s3((struct S3){ { 1, 2, 3 } }, 4); printf("%d %d %d\n", s3.c[0], s3.c[1], s3.c[2]);
  struct S8 s8 = f_s8((struct S8){ 7, 1.5f }, 3); printf("%d %g\n", s8.a, s8.f);
  struct S12 s12 = f_s12(9, (struct S12){ 1, 2, 3 }); printf("%d %d %d\n", s12.a, s12.b, s12.c);
  struct S16 s16 = f_s16((struct S16){ 11, 2.5 }, 0.25); printf("%ld %g\n", s16.a, s16.d);
  struct SFF sff = f_sff(0.5f, (struct SFF){ 1, 2 }); printf("%g %g\n", sff.a, sff.b);
  struct SDD sdd = f_sdd((struct SDD){ 1, 2 }, (struct SDD){ 10, 20 }); printf("%g %g\n", sdd.a, sdd.b);
  struct SFI sfi = f_sfi((struct SFI){ 1.5f, 10, 2.25 }, 2); printf("%g %d %g\n", sfi.f, sfi.i, sfi.d);
  struct S24 s24 = f_s24((struct S24){ 1, 2, 3 }, 7); printf("%ld %ld %ld\n", s24.a, s24.b, s24.c);
  struct SLD sld = f_sld((struct SLD){ 1.5L }, 3.0L); printf("%Lg\n", sld.v);
  struct SMIX sm = f_smix(2, (struct SMIX){ 'a', 9.0L, 21 }); printf("%c %Lg %d\n", sm.c, sm.v, sm.i);
  struct P9 p9 = f_p9((struct P9){ 'a', 1L << 40 }, 2); printf("%c %ld\n", p9.c, p9.l);
  struct P5 p5 = f_p5(1, (struct P5){ 'z', -100 }); printf("%c %d\n", p5.c, p5.i);
  struct BF bf = f_bf((struct BF){ 5, -60, 1UL << 35 }, 2); printf("%u %d %lu\n", bf.a, bf.b, (unsigned long)bf.c);
  union U u = { .d = 1.0 }; u.c[8] = 1; u = f_u(u, 9); printf("%d %d\n", u.i, u.c[8]);
  struct NEST n = f_nest((struct NEST){ { 1, 2.0f }, { 3.0f, 4.0f } }, (struct ARR){ { 0.5f, 0.25f }, { 1, 2, 3, 4, 5 } });
  printf("%d %g %g %g\n", n.s.a, n.s.f, n.f.a, n.f.b);
  struct ARR a = f_arr((struct ARR){ { 1, 2 }, "abcd" }, 6); printf("%g %g %s %d\n", a.f[0], a.f[1], a.c, a.c[4]);
  printf("%.10g\n", f_many(1, 2.5, 3, 4.5f, 5, 6.5, 7, 8.5, 9, 10.5, 11, 12.5, 13, 14.5, 15, 16.5, 17.5f, 18.5, 19.5L, (struct S16){ 20, 21.5 }, (struct SFF){ 22.5f, 23.5f }, 24));
  printf("%Lg\n", f_va("ildLsfbpLidddddddddiiiiiiisL", 1, 2L, 3.5, 4.25L, (struct S16){ 5, 6.5 }, (struct SFF){ 7.5f, 8.5f }, (struct S24){ 9, 10, 11 }, (struct P9){ 12, 13 }, 0.125L, 14,
                  1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 1, 2, 3, 4, 5, 6, 7, (struct S16){ 100, 0.5 }, 1e3L));
  printf("%ld\n", f_fp(cb_s16, (struct S16){ 3, 4.75 }));
  return 0;
}
