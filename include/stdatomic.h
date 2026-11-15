#ifndef __STDATOMIC_H
#define __STDATOMIC_H
/* C11 atomics, on the __atomic builtins of builtins.h. Every operation is
   sequentially consistent; the _explicit forms ignore their memory orders. */

typedef enum {
  memory_order_relaxed, memory_order_consume, memory_order_acquire,
  memory_order_release, memory_order_acq_rel, memory_order_seq_cst
} memory_order;

#define ATOMIC_BOOL_LOCK_FREE 2
#define ATOMIC_CHAR_LOCK_FREE 2
#define ATOMIC_CHAR16_T_LOCK_FREE 2
#define ATOMIC_CHAR32_T_LOCK_FREE 2
#define ATOMIC_WCHAR_T_LOCK_FREE 2
#define ATOMIC_SHORT_LOCK_FREE 2
#define ATOMIC_INT_LOCK_FREE 2
#define ATOMIC_LONG_LOCK_FREE 2
#define ATOMIC_LLONG_LOCK_FREE 2
#define ATOMIC_POINTER_LOCK_FREE 2

typedef _Atomic(_Bool) atomic_bool;
typedef _Atomic(char) atomic_char;
typedef _Atomic(signed char) atomic_schar;
typedef _Atomic(unsigned char) atomic_uchar;
typedef _Atomic(short) atomic_short;
typedef _Atomic(unsigned short) atomic_ushort;
typedef _Atomic(int) atomic_int;
typedef _Atomic(unsigned int) atomic_uint;
typedef _Atomic(long) atomic_long;
typedef _Atomic(unsigned long) atomic_ulong;
typedef _Atomic(long long) atomic_llong;
typedef _Atomic(unsigned long long) atomic_ullong;
typedef _Atomic(unsigned short) atomic_char16_t;
typedef _Atomic(unsigned int) atomic_char32_t;
typedef _Atomic(int) atomic_wchar_t;
typedef _Atomic(signed char) atomic_int_least8_t;
typedef _Atomic(unsigned char) atomic_uint_least8_t;
typedef _Atomic(short) atomic_int_least16_t;
typedef _Atomic(unsigned short) atomic_uint_least16_t;
typedef _Atomic(int) atomic_int_least32_t;
typedef _Atomic(unsigned int) atomic_uint_least32_t;
typedef _Atomic(long) atomic_int_least64_t;
typedef _Atomic(unsigned long) atomic_uint_least64_t;
typedef _Atomic(signed char) atomic_int_fast8_t;
typedef _Atomic(unsigned char) atomic_uint_fast8_t;
typedef _Atomic(long) atomic_int_fast16_t;
typedef _Atomic(unsigned long) atomic_uint_fast16_t;
typedef _Atomic(long) atomic_int_fast32_t;
typedef _Atomic(unsigned long) atomic_uint_fast32_t;
typedef _Atomic(long) atomic_int_fast64_t;
typedef _Atomic(unsigned long) atomic_uint_fast64_t;
typedef _Atomic(long) atomic_intptr_t;
typedef _Atomic(unsigned long) atomic_uintptr_t;
typedef _Atomic(unsigned long) atomic_size_t;
typedef _Atomic(long) atomic_ptrdiff_t;
typedef _Atomic(long) atomic_intmax_t;
typedef _Atomic(unsigned long) atomic_uintmax_t;

#define ATOMIC_VAR_INIT(v) (v)
#define atomic_init(p, v) ((void)(*(p) = (v)))
#define kill_dependency(v) (v)
#define atomic_is_lock_free(p) (sizeof(*(p)) <= 8)
#define atomic_thread_fence(o) ((void)({ __asm__ __volatile__("mfence"); 0; }))
#define atomic_signal_fence(o) ((void)0)

#define atomic_load(p) __atomic_load_n(p, 5)
#define atomic_load_explicit(p, o) __atomic_load_n(p, o)
#define atomic_store(p, v) __atomic_store_n(p, v, 5)
#define atomic_store_explicit(p, v, o) __atomic_store_n(p, v, o)
#define atomic_exchange(p, v) __atomic_exchange_n(p, v, 5)
#define atomic_exchange_explicit(p, v, o) __atomic_exchange_n(p, v, o)
#define atomic_compare_exchange_strong(p, e, d) __atomic_compare_exchange_n(p, e, d, 0, 5, 5)
#define atomic_compare_exchange_strong_explicit(p, e, d, o, o2) __atomic_compare_exchange_n(p, e, d, 0, o, o2)
#define atomic_compare_exchange_weak(p, e, d) __atomic_compare_exchange_n(p, e, d, 1, 5, 5)
#define atomic_compare_exchange_weak_explicit(p, e, d, o, o2) __atomic_compare_exchange_n(p, e, d, 1, o, o2)
#define atomic_fetch_add(p, v) __atomic_fetch_add(p, v, 5)
#define atomic_fetch_add_explicit(p, v, o) __atomic_fetch_add(p, v, o)
#define atomic_fetch_sub(p, v) __atomic_fetch_sub(p, v, 5)
#define atomic_fetch_sub_explicit(p, v, o) __atomic_fetch_sub(p, v, o)
#define atomic_fetch_and(p, v) __atomic_fetch_and(p, v, 5)
#define atomic_fetch_and_explicit(p, v, o) __atomic_fetch_and(p, v, o)
#define atomic_fetch_or(p, v) __atomic_fetch_or(p, v, 5)
#define atomic_fetch_or_explicit(p, v, o) __atomic_fetch_or(p, v, o)
#define atomic_fetch_xor(p, v) __atomic_fetch_xor(p, v, 5)
#define atomic_fetch_xor_explicit(p, v, o) __atomic_fetch_xor(p, v, o)

typedef struct { _Bool __v; } atomic_flag;
#define ATOMIC_FLAG_INIT { 0 }
#define atomic_flag_test_and_set(p) __atomic_exchange_n(&(p)->__v, 1, 5)
#define atomic_flag_test_and_set_explicit(p, o) __atomic_exchange_n(&(p)->__v, 1, o)
#define atomic_flag_clear(p) __atomic_store_n(&(p)->__v, 0, 5)
#define atomic_flag_clear_explicit(p, o) __atomic_store_n(&(p)->__v, 0, o)
#endif
