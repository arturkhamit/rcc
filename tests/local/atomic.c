// <stdatomic.h>: every operation at every width, and a counter shared by threads.
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>

static atomic_long counter;
static atomic_flag lock = ATOMIC_FLAG_INIT;
static long guarded; // protected by `lock`
static _Atomic(uint32_t) bits;

// Plain operators on atomic objects are atomic too.
static _Atomic int plus, minus, mixed;
static _Atomic unsigned toggled; // only xor: the order of the threads does not matter
static _Atomic unsigned char small;
static _Atomic(long) *indirect = &counter;
static struct { char pad; _Atomic short field; } in_struct;
static _Atomic double real;
static atomic_int last;

static void *worker(void *arg) {
  long id = (long)arg;
  for (int i = 0; i < 100000; i++) {
    plus++;
    --minus;
    mixed += 3;
    mixed -= 1;
    toggled ^= 1u << id;
    small += 1;
    *indirect += 2;
    in_struct.field++;
    // (`real += 0.5` would be atomic too, but gcc needs libatomic to link it.)
    double seen = atomic_load(&real);
    while (!atomic_compare_exchange_weak(&real, &seen, seen + 0.5)) {}
    last = (int)id;
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
  printf("operators: %d %d %d %x %d %d %g %d\n", plus, minus, mixed, toggled, small, in_struct.field, real, last >= 0 && last < 4);

  // The values of the operator expressions, and pointers.
  _Atomic int v = 10;
  int r1 = v++, r2 = ++v, r3 = (v += 5), r4 = (v = 100), r5 = v--;
  printf("values: %d %d %d %d %d %d\n", r1, r2, r3, r4, r5, v);
  int arr[4] = {1, 2, 3, 4};
  _Atomic(int *) ap = arr;
  ap += 2;
  int *prev = ap++;
  printf("pointer: %d %d\n", *prev, *ap);
  _Atomic _Bool ab = 0;
  ab |= 1;
  _Atomic float af = 1.5f;
  af = af * 2;
  printf("bool, float: %d %g\n", ab, af);
  // typeof drops _Atomic; a qualified pointee is a distinct type.
  __typeof__(v) plain = 7;
  plain++;
  const char *cs = "";
  printf("types: %d %d %d %d\n", plain, _Generic(cs, char *: 1, const char *: 2), _Generic(&v, int *: 1, _Atomic int *: 2),
         __builtin_types_compatible_p(const int, int));
  return 0;
}
