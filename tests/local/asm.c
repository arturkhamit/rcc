// Inline assembly with operands (x86-64). Compared with gcc.
#include <stdio.h>
#include <string.h>

static int g = 40;
struct pair { int a; long b; };

static unsigned long mulhi(unsigned long a, unsigned long b) {
  unsigned long lo, hi;
  __asm__("mulq %[m]" : "=a"(lo), "=d"(hi) : "a"(a), [m] "rm"(b) : "cc");
  return hi + (lo & 1);
}

static int cas(long *p, long *expected, long desired) {
  unsigned char ok;
  __asm__ __volatile__("lock cmpxchgq %3, %1\n\tsete %0" : "=q"(ok), "+m"(*p), "+a"(*expected) : "r"(desired) : "cc", "memory");
  return ok;
}

int main(void) {
  // Fixed registers: cpuid leaf 0 gives the vendor string in ebx, edx, ecx.
  unsigned a, b, c, d;
  char vendor[13] = {0};
  __asm__("cpuid" : "=a"(a), "=b"(b), "=c"(c), "=d"(d) : "a"(0), "c"(0));
  memcpy(vendor, &b, 4), memcpy(vendor + 4, &d, 4), memcpy(vendor + 8, &c, 4);
  printf("cpuid: %s, max leaf > 0: %d\n", vendor, a > 0);

  // Any register, in-out, immediates, a symbolic operand name.
  int x = 5, y = 7;
  __asm__("addl %1, %0" : "+r"(x) : "r"(y));
  __asm__("addl %[k], %[v]" : [v] "+r"(x) : [k] "i"(100));
  printf("add: %d\n", x);

  // Memory operands: a local, a global, through a pointer, a struct member.
  int *p = &y;
  struct pair s = {1, 2}, *sp = &s;
  __asm__("incl %0" : "+m"(x));
  __asm__("incl %0" : "+m"(g));
  __asm__("addl $10, %0" : "+m"(*p));
  __asm__("addq $20, %0" : "+m"(sp->b));
  printf("mem: %d %d %d %ld\n", x, g, y, s.b);

  // Operand sizes and the size modifiers.
  unsigned char c8 = 0xf0;
  unsigned short c16 = 0xff00;
  unsigned long c64 = 0x123456789abcdef0;
  __asm__("shrb $4, %0" : "+r"(c8));
  __asm__("shrw $4, %0" : "+q"(c16));
  __asm__("rorq $8, %0" : "+r"(c64));
  printf("sizes: %x %x %lx\n", c8, c16, c64);
  unsigned long wide = 0x1111111122223344;
  unsigned r1, r2, r3;
  __asm__("movzbl %b1, %0" : "=r"(r1) : "r"(wide));
  __asm__("movzwl %w1, %0" : "=r"(r2) : "r"(wide));
  __asm__("movzbl %h1, %0" : "=r"(r3) : "a"(wide));
  __asm__("movl %k1, %k0" : "=&r"(wide) : "r"(0x5555555566667777UL));
  printf("modifiers: %x %x %x %lx\n", r1, r2, r3, wide);

  // An input tied to an output.
  long t;
  __asm__("negq %0" : "=r"(t) : "0"(1234L));
  printf("tied: %ld\n", t);

  // A callee-saved register in the clobber list, and one used by an operand.
  long keep;
  __asm__("movq $77, %%rbx\n\tmovq %%rbx, %0" : "=r"(keep) : : "rbx");
  long viab = 5;
  __asm__("leaq 1(%0, %0, 2), %0" : "+b"(viab));
  printf("rbx: %ld %ld\n", keep, viab);

  // Fixed registers of a string instruction.
  char src[] = "inline assembly", dst[32] = {0};
  char *dp = dst, *sp2 = src;
  unsigned long n = sizeof src;
  __asm__("rep movsb" : "+D"(dp), "+S"(sp2), "+c"(n) : : "memory");
  printf("movsb: %s (%ld left, advanced %ld)\n", dst, n, (long)(dp - dst));

  printf("mulhi: %lx\n", mulhi(0xfedcba9876543210, 0x1000000000000001));

  // Compare-and-swap: success, then failure that reports the current value.
  long v = 10, e = 10;
  int ok1 = cas(&v, &e, 20);
  int ok2 = cas(&v, &e, 30);
  printf("cas: %d %d v=%ld e=%ld\n", ok1, ok2, v, e);

  // The lvalue of an in-out operand is evaluated once.
  int arr[3] = {1, 2, 3}, *q = arr;
  __asm__("shll $4, %0" : "+r"(*q++));
  printf("once: %d %d %d, %ld\n", arr[0], arr[1], arr[2], (long)(q - arr));

  // In an expression, with temporaries of the enclosing expression live.
  int sum = x + ({ int z = 3; __asm__("imull %1, %0" : "+r"(z) : "r"(y)); z; }) * 2;
  printf("nested: %d\n", sum);

  // No operands but a clobber list; the empty template as an optimization barrier.
  __asm__ __volatile__("nop" : : : "memory");
  double dbl = 1.5;
  __asm__ __volatile__("" : "+r"(dbl));
  __asm__ __volatile__("" : : "r,m"(s) : "memory");
  printf("barrier: %g %d\n", dbl, s.a);
  return 0;
}
