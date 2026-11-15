// <stdatomic.h>: every operation at every width, and a counter shared by threads.
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>

static atomic_long counter;
static atomic_flag lock = ATOMIC_FLAG_INIT;
static long guarded; // protected by `lock`
static _Atomic(uint32_t) bits;

static void *worker(void *arg) {
  long id = (long)arg;
  for (int i = 0; i < 100000; i++) {
    atomic_fetch_add(&counter, 1);
    atomic_fetch_xor(&bits, 1u << id);
    while (atomic_flag_test_and_set(&lock)) {}
    guarded += id;
    atomic_flag_clear(&lock);
  }
  return 0;
}

#define WIDTH(T) do { \
    _Atomic(T) v = 100; T e = 100, old; \
    old = atomic_fetch_add(&v, 5); printf(" %d,%d", (int)old, (int)atomic_load(&v)); \
    old = atomic_fetch_sub(&v, 3); printf(" %d,%d", (int)old, (int)v); \
    old = atomic_fetch_or(&v, 0x18); printf(" %d,%d", (int)old, (int)v); \
    old = atomic_fetch_and(&v, 0x6c); printf(" %d,%d", (int)old, (int)v); \
    old = atomic_fetch_xor(&v, 0x7f); printf(" %d,%d", (int)old, (int)v); \
    old = atomic_exchange(&v, 100); printf(" %d,%d", (int)old, (int)v); \
    int ok = atomic_compare_exchange_strong(&v, &e, 42); printf(" cas %d %d %d", ok, (int)v, (int)e); \
    ok = atomic_compare_exchange_weak(&v, &e, 7); printf(" %d %d %d", ok, (int)v, (int)e); \
    atomic_store(&v, (T)-1); printf(" store %lld\n", (long long)atomic_load(&v)); \
  } while (0)

int main(void) {
  printf("u8: "); WIDTH(uint8_t);
  printf("i16:"); WIDTH(int16_t);
  printf("u32:"); WIDTH(uint32_t);
  printf("i64:"); WIDTH(int64_t);

  // Values that need the full width, and the neighbours of the object stay intact.
  struct { uint16_t before; _Atomic(uint16_t) v; uint16_t after; } s = {0xaaaa, 0xfff0, 0xbbbb};
  printf("wrap: %x", atomic_fetch_add(&s.v, 0x20));
  printf(" %x %x %x\n", s.before, atomic_load(&s.v), s.after);
  atomic_ullong big = 0xffffffff00000000;
  atomic_fetch_add_explicit(&big, 0x100000001, memory_order_relaxed);
  printf("big: %llx\n", atomic_load_explicit(&big, memory_order_acquire));

  // Pointers.
  int a = 1, b = 2;
  _Atomic(int *) p = &a;
  int *expect = &a;
  int swapped = atomic_compare_exchange_strong(&p, &expect, &b);
  int *was = atomic_exchange(&p, &a);
  printf("ptr: %d %d %d\n", swapped, *was, *atomic_load(&p));

  // Nested uses and operands with side effects are evaluated once.
  atomic_int x = 1, y = 10;
  int n = 0;
  atomic_fetch_add(&x, atomic_fetch_add(&y, (n++, 5)));
  printf("nested: %d %d %d\n", atomic_load(&x), atomic_load(&y), n);

  atomic_bool flag;
  atomic_init(&flag, 0);
  int was_set = atomic_exchange(&flag, 1);
  printf("bool: %d %d\n", was_set, atomic_load(&flag));
  atomic_thread_fence(memory_order_seq_cst);
  printf("lock free: %d %d\n", (int)atomic_is_lock_free(&x), ATOMIC_INT_LOCK_FREE);

  pthread_t t[4];
  for (long i = 0; i < 4; i++) pthread_create(&t[i], 0, worker, (void *)i);
  for (int i = 0; i < 4; i++) pthread_join(t[i], 0);
  printf("threads: %ld %ld %x\n", atomic_load(&counter), guarded, atomic_load(&bits));
  return 0;
}
