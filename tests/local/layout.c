#include <stdio.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <stdalign.h>

struct __attribute__((packed)) P1 { char c; int i; short s; double d; };
struct P2 { char c; int i; long l; } __attribute__((packed));
struct P3 { char c; int i __attribute__((packed)); short s; };
struct A1 { char c; int i __attribute__((aligned(16))); };
struct A2 { char c; } __attribute__((aligned(8)));
struct A3 { char c; _Alignas(8) char d; _Alignas(double) char e; };
struct __attribute__((packed)) Hdr { unsigned int ihl : 4; unsigned int version : 4; uint8_t tos; uint16_t len; uint32_t addr; };
struct __attribute__((packed)) PB { char c; int f : 5; int g : 3; unsigned h : 12; short s; };
struct __attribute__((packed, aligned(4))) PA { char c; long l; };
typedef struct { char c; long l; } __attribute__((packed)) TP;
typedef struct { int x; } TA __attribute__((aligned(16)));

#pragma pack(push, 1)
struct K1 { char c; int i; double d; };
#pragma pack(2)
struct K2 { char c; int i; double d; };
#pragma pack(pop)
struct K3 { char c; int i; double d; };
#pragma pack(4)
struct K4 { char c; double d; short s; };
#pragma pack()
struct K5 { char c; double d; short s; };

_Alignas(32) char gbuf[3];
static __attribute__((aligned(64))) int gal;
int __attribute__((aligned(16))) gal2 = 5;
_Thread_local int tl1 = 7;
static __thread long tl2;
__thread char tlbuf[10] = "tls";
extern __thread int tl1;

#define SHOW(T) printf(#T ": size %zu align %zu\n", sizeof(struct T), _Alignof(struct T))
#define OFF(T, m) printf(#T "." #m " at %zu\n", offsetof(struct T, m))

int *tlp(void) { return &tl1; }

int main(void) {
  SHOW(P1); OFF(P1, i); OFF(P1, s); OFF(P1, d);
  SHOW(P2); OFF(P2, l);
  SHOW(P3); OFF(P3, i); OFF(P3, s);
  SHOW(A1); OFF(A1, i);
  SHOW(A2);
  SHOW(A3); OFF(A3, d); OFF(A3, e);
  SHOW(Hdr); OFF(Hdr, tos); OFF(Hdr, len); OFF(Hdr, addr);
  SHOW(PB); OFF(PB, s);
  SHOW(PA); OFF(PA, l);
  SHOW(K1); OFF(K1, i); OFF(K1, d);
  SHOW(K2); OFF(K2, i); OFF(K2, d);
  SHOW(K3); OFF(K3, i); OFF(K3, d);
  SHOW(K4); OFF(K4, d); OFF(K4, s);
  SHOW(K5); OFF(K5, d); OFF(K5, s);
  printf("TP %zu %zu TA %zu %zu\n", sizeof(TP), _Alignof(TP), sizeof(TA), _Alignof(TA));

  struct Hdr h; memset(&h, 0, sizeof h);
  h.ihl = 5; h.version = 4; h.tos = 0xab; h.len = 0x1234; h.addr = 0xdeadbeef;
  unsigned char *b = (unsigned char *)&h;
  for (size_t i = 0; i < sizeof h; i++) printf("%02x", b[i]);
  printf(" %d %d\n", h.ihl, h.version);
  struct PB pb = { 'x', -7, 3, 0xabc, -2 };
  pb.f += 2; pb.h ^= 0xfff; pb.g--;
  b = (unsigned char *)&pb;
  for (size_t i = 0; i < sizeof pb; i++) printf("%02x", b[i]);
  printf(" %d %d %u %d\n", pb.f, pb.g, pb.h, pb.s);
  struct P1 p1 = { 'a', 0x11223344, 0x5566, 2.5 }, p1b = p1;
  p1b.i++; p1b.d *= 2;
  printf("%d %x %x %g\n", p1b.c, p1b.i, p1b.s, p1b.d);
  static struct K1 ks = { 1, 2, 3.0 };
  printf("%d %d %g\n", ks.c, ks.i, ks.d);

  _Alignas(16) char lbuf[5]; alignas(8) char lb2;
  printf("%d %d %d %d %d\n", (int)((uintptr_t)gbuf % 32), (int)((uintptr_t)&gal % 64), (int)((uintptr_t)&gal2 % 16), (int)((uintptr_t)lbuf % 16), (int)((uintptr_t)&lb2 % 8));
  tl1 += 3; tl2 = tl1 * 2; (*tlp())++;
  printf("%d %ld %s %d\n", tl1, tl2, tlbuf, tlp() == &tl1);
  return 0;
}
