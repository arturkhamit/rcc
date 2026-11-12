#include <stdio.h>
int order[8], n;
static void __attribute__((constructor)) init1(void) { order[n++] = 1; }
__attribute__((destructor)) static void fini1(void) { printf("fini %d\n", n); }
int real_fn(int x) { return x * 2; }
int alias_fn(int) __attribute__((alias("real_fn")));
extern int weak_undef(void) __attribute__((weak));
int __attribute__((weak)) weak_def(void) { return 5; }
int renamed(int x) asm("renamed_impl");
int renamed(int x) { return x + 100; }
extern int renamed_impl(int);
int secvar __attribute__((section(".mydata"))) = 77;
static int hidden_fn(void) __attribute__((visibility("hidden")));
static int hidden_fn(void) { return 3; }
asm(".globl asm_sym\n.data\nasm_sym: .long 4242\n.text");
extern int asm_sym;
int main(void) {
  printf("%d %d\n", n, order[0]);
  printf("%d %d %d\n", alias_fn(21), weak_undef ? 1 : 0, weak_def());
  printf("%d %d %d %d %d\n", renamed(1), renamed_impl(2), secvar, hidden_fn(), asm_sym);
  asm("nop");
  int z = 0, w = 7;
  printf("%d %d\n", z ?: 9, w ?: 9);
  printf("%zu %zu\n", __alignof__ w, sizeof (int[3]){ 1, 2, 3 });
  return 0;
}
