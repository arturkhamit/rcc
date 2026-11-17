# rcc — a C compiler written in Rust

`rcc` compiles C11 and the commonly used GNU extensions to x86-64 Linux
assembly (System V ABI). It has its own preprocessor, lexer, parser, type
checker and code generator — no dependencies besides the Rust standard library.
The system `as` and `cc` are used only to assemble and link.

## Build and use

    cargo build --release
    ./target/release/rcc hello.c -o hello && ./hello
    ./target/release/rcc -c a.c b.c          # object files
    ./target/release/rcc -S a.c              # assembly
    ./target/release/rcc -E a.c              # preprocess only

It works as a drop-in `CC` for ordinary build systems (`make CC=rcc`,
`CC=rcc ./configure`). Options: `-o -c -S -E -I -isystem -iquote -idirafter -D -U
-include -x c -l -L -Wl, -Xlinker -shared -static -pthread -fPIC`, dependency
output (`-MD -MMD -MF -MT -MQ -MP -M -MM`), `-` for standard input, `--version`,
`-dumpmachine`. `-O1` and higher turn the optimizer on (all levels are the
same); without it, or with `-O0`, the code is straightforward and compiles
fastest. `-g` adds line numbers and unwind information (source lines and
backtraces in gdb; no variables). Other flags (`-W...`, `-std=...`, `-f...`,
`-m...`) are accepted and ignored. Use `-fPIC` for code that goes into a shared
library.

## Status

| Check | Result |
|-------|--------|
| [c-testsuite](https://github.com/c-testsuite/c-testsuite) and local tests (compared with gcc) | 234 / 234 |
| gcc.c-torture/execute (GCC's own torture tests) | 1569 / 1701 |
| ABI: objects from rcc and gcc calling each other (`tests/interop`) | passes |
| Lua 5.4.7, official test suite (`all.lua`) | passes |
| SQLite 3.46 amalgamation | same query output as a gcc build |
| zlib 1.3.1 (`./configure`, static and shared, `make test`) | passes |
| bzip2 1.0.8 (`make`, including its self-test) | passes |
| lz4 1.10.0 (command-line tests, frame test, fuzzer) | passes |
| zstd 1.5.6 (command-line tests, unit tests, fuzzer) | passes |
| QuickJS, with Temporal and Intl (`make test`) | passes |
| chibicc built with rcc, its test suite | passes |
| tinycc built with rcc: compiles and runs programs and itself | passes |
| tinycc's `tcctest.c` (without its inline-assembly part) | output equals gcc's, apart from three `__builtin_constant_p` lines only an optimizer folds |

Of the torture tests that do not pass, 119 are rejected at compile time (nested
functions, vector and complex types, `__builtin_setjmp` and the like)
and 13 give a wrong result, on GNU-specific behaviour (bitfields wider than 32
bits evaluated in their own width, `scalar_storage_order`,
`-finstrument-functions`, const-qualified type compatibility).

Every check above gives the same result with the optimizer on and off.

Speed of the generated code (`tests/bench.sh`: seconds for the same workload,
best of three runs; lower is better):

| Workload | rcc | rcc -O1 | gcc -O0 | gcc -O2 |
|---|---|---|---|---|
| bzip2, compressing 9 MB of text | 0.95 | 0.53 | 0.82 | 0.46 |
| lz4 -9, the same | 1.09 | 0.38 | 0.59 | 0.27 |
| zstd -9, the same | 0.89 | 0.34 | 0.49 | 0.14 |
| Lua: recursion, table sort, string formatting | 3.13 | 1.88 | 2.32 | 1.35 |
| SQLite: inserts, index, joins, aggregates | 1.10 | 0.54 | 1.34 | 0.43 |

Tests:

    ./fetch_tests.sh && ./run_tests.sh        # c-testsuite, local tests, gcc interop
    ./fetch_tests.sh all                      # also the corpora for the three below
    tests/torture.sh tests/ext/gcc-ts/gcc/testsuite/gcc.c-torture/execute
    tests/real.sh                             # Lua, SQLite, zlib, bzip2, lz4, zstd, QuickJS, chibicc, tinycc
    tests/bench.sh                            # the table above

The test scripts compile as the programs' own build systems ask (mostly with
`-O2`). `RCC_OPT=1` in the environment turns the optimizer on whatever the
flags, `RCC_OPT=0` off: `RCC_OPT=0 tests/real.sh` checks the simple code
generator on everything. `RCC_STATS=1` lists the functions the optimizer left
to the simple generator, and why.

## What is supported

- C11: all statements and expressions, structs, unions, bitfields, enums,
  designated initializers, compound literals, variable-length arrays (also as
  parameters: `void f(int n, double a[n][n])`), `_Generic`, `_Alignas`,
  `_Alignof`, `_Static_assert`, `_Thread_local`, `_Noreturn`, variadic
  functions, K&R-style definitions.
- `__int128` and `unsigned __int128` (`__int128_t`, `__uint128_t`), passed and
  returned as gcc does. Their operators are calls: small functions for
  addition, comparison and the like (inlined with `-O`), libgcc for
  multiplication, division, shifts and conversions to and from floating point.
- Atomics: `_Atomic` objects, whose operators (`x++`, `x += n`, `x = v`) are
  atomic, and `<stdatomic.h>` with the `atomic_*` functions and `atomic_flag`.
  Lock-free (`lock`-prefixed instructions) for objects of up to 8 bytes.
- `long double` is the real x87 80-bit type; its constants keep the full 64-bit mantissa.
- System V calling convention in full: structs in registers and in memory,
  `long double`, packed structs, varargs; verified against gcc in both directions.
- GNU extensions: statement expressions, `typeof`, `alloca`, labels as values
  and computed `goto`, `__label__`, case ranges, range designators, `a ?: b`,
  zero-length and flexible array members (also in static initializers), empty
  structs, compound literals in static initializers, `#pragma pack`,
  `#pragma once`, `#include_next`, `__COUNTER__`, …
- Inline assembly with operands: `asm("cpuid" : "=a"(x), "=b"(y) : "a"(leaf))`.
  Operands in general registers (`r`, `q`, `a` `b` `c` `d` `S` `D`), in memory
  (`m`) and immediate (`i`); in-out (`+`) and tied (`"0"`) operands, named
  operands (`%[name]`), the size modifiers `%b0 %w0 %k0 %q0 %h0`, clobber lists.
- Attributes: `packed`, `aligned`, `mode`, `weak`, `alias`, `section`,
  `constructor`, `destructor`, `visibility`, asm labels (`int x asm("y")`);
  others are accepted and ignored.
- Builtins: `__builtin_va_*`, `offsetof`, `expect`, `constant_p`, `choose_expr`,
  `types_compatible_p`, `classify_type`, `clz/ctz/popcount/parity/clrsb/bswap`,
  `add/sub/mul_overflow` (all variants), `frame_address`, `return_address`,
  `unreachable`, `trap`, float classification (`isnan`, `isinf`, …),
  `__builtin_<libc function>`, and `__atomic_load_n/store_n/exchange_n/
  compare_exchange_n/fetch_add/sub/and/or/xor`.
- Code that a constant condition makes unreachable (`if (0) f();`) is not
  emitted, so it may refer to functions that do not exist.

## Design

    src/lex.rs         tokenizer (shared by the preprocessor and the parser)
    src/preprocess.rs  macros (hideset algorithm), #if, #include, built-in headers
    src/types.rs       C type representation, ABI classification helpers
    src/parse.rs       recursive-descent parser -> typed AST, constant evaluation, initializers
    src/codegen.rs     AST -> x86-64 assembly (stack machine, System V calling convention)
    src/opt.rs         removal of unreachable code from the generated assembly
    src/ir.rs          -O: AST -> three-address code over virtual registers, in basic blocks
    src/pass.rs        -O: inlining, constant folding and propagation, common
                       subexpressions, dead code, control-flow cleanup
    src/x86.rs         -O: register allocation (linear scan) and x86-64 emission
    src/main.rs        driver
    include/           built-in headers (stddef.h, stdarg.h, stdatomic.h, ...) and
                       builtins.h, the GNU builtins that can be written in C

Without `-O` the structure follows Rui Ueyama's
[chibicc](https://github.com/rui314/chibicc): every expression is evaluated
into `%rax` / `%xmm0` / `%st(0)`, with the machine stack for temporaries when an
operand is not a constant or a variable.

With `-O` a function is first lowered to an intermediate representation, where
scalar variables whose address is not taken are virtual registers. Small
functions are inlined into their callers; constants and copies are propagated
and folded; a computation or a load that was already done on the way is reused;
what nothing uses is removed. Registers are then allocated by linear scan (the
least used values go to the frame when registers run out), and a dense `switch`
becomes a jump table. A function that uses something this path does not handle
(a variadic definition, `long double`, `alloca` or a VLA, inline assembly, a
computed `goto`, a call to `setjmp`) is compiled the simple way; the two kinds
of code mix freely, as both follow the ABI. `volatile` objects are always read
and written exactly as the source says.

`__GNUC__` is deliberately not defined: glibc's headers then avoid GNU-only
constructs.

## Not supported

- in inline assembly: operands in SSE or x87 registers (`"x"`, `"t"`), flag
  outputs (`"=@ccz"`), `asm goto`, `{att|intel}` alternatives, and local
  register variables (`register long r asm("r10")`)
- `_Complex`, vector types, nested functions, `__builtin_setjmp`
- for 128-bit integers: constants wider than 64 bits anywhere but in the
  initializer of a static 128-bit object, `case` labels wider than 64 bits,
  `va_arg`, bit-fields, and the overflow builtins on 128-bit operands
- atomic objects larger than 8 bytes (structs, `long double`) are read and
  written like plain ones
- qualifiers are part of the types (`_Generic` tells `char *` from
  `const char *`), but there are no diagnostics for writes to `const` objects,
  and `long`/`long long` (and `char`/`signed char`) are one type to `_Generic`
- constant expressions of type `long double` in static initializers are
  folded in double precision (plain constants are exact)
- locals cannot be aligned to more than 16 bytes
- few diagnostics for invalid programs: rcc assumes its input compiles with
  gcc or clang, and stops at the first error
- debug info is line numbers only (no variables or types); with `-O`, a
  breakpoint on a function that was inlined does not trigger
- the optimizer does no loop optimizations (hoisting, strength reduction,
  vectorization), and inlines only small functions: `always_inline` is not seen,
  since libraries request it only from compilers that define `__GNUC__`
