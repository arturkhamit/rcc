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
`-dumpmachine`. Other flags (`-O2`, `-g`, `-W...`, `-std=...`, `-f...`, `-m...`)
are accepted and ignored. Use `-fPIC` for code that goes into a shared library.

## Status

| Check | Result |
|-------|--------|
| [c-testsuite](https://github.com/c-testsuite/c-testsuite) and local tests (compared with gcc) | 229 / 231 |
| gcc.c-torture/execute (GCC's own torture tests) | 1567 / 1701 |
| ABI: objects from rcc and gcc calling each other (`tests/interop`) | passes |
| Lua 5.4.7, official test suite (`all.lua`) | passes |
| SQLite 3.46 amalgamation | same query output as a gcc build |
| zlib 1.3.1 (`./configure`, static and shared, `make test`) | passes |
| bzip2 1.0.8 (`make`, including its self-test) | passes |
| chibicc built with rcc, its test suite | passes |
| tinycc built with rcc: compiles and runs programs and itself | passes |
| tinycc's `tcctest.c` (without its inline-assembly part) | output equals gcc's, apart from three `__builtin_constant_p` lines only an optimizer folds |

Of the torture tests that do not pass, 121 are rejected at compile time (nested
functions, vector, complex and 128-bit types, `__builtin_setjmp` and the like)
and 13 give a wrong result, on GNU-specific behaviour (bitfields wider than 32
bits evaluated in their own width, `scalar_storage_order`,
`-finstrument-functions`, const-qualified type compatibility).

Speed of the generated code is that of `gcc -O0` or somewhat better; there is
no optimizer beyond dead-code removal and operand selection.

Tests:

    ./fetch_tests.sh && ./run_tests.sh        # c-testsuite, local tests, gcc interop
    ./fetch_tests.sh all                      # also the corpora for the two below
    tests/torture.sh tests/ext/gcc-ts/gcc/testsuite/gcc.c-torture/execute
    tests/real.sh                             # Lua, SQLite, zlib, bzip2, chibicc, tinycc

## What is supported

- C11: all statements and expressions, structs, unions, bitfields, enums,
  designated initializers, compound literals, variable-length arrays (also as
  parameters: `void f(int n, double a[n][n])`), `_Generic`, `_Alignas`,
  `_Alignof`, `_Static_assert`, `_Thread_local`, `_Noreturn`, variadic
  functions, K&R-style definitions.
- `long double` is the real x87 80-bit type; its constants keep the full 64-bit mantissa.
- System V calling convention in full: structs in registers and in memory,
  `long double`, packed structs, varargs; verified against gcc in both directions.
- GNU extensions: statement expressions, `typeof`, `alloca`, labels as values
  and computed `goto`, `__label__`, case ranges, range designators, `a ?: b`,
  zero-length and flexible array members (also in static initializers), empty
  structs, `#pragma pack`, `#pragma once`, `#include_next`, `__COUNTER__`, …
- Attributes: `packed`, `aligned`, `mode`, `weak`, `alias`, `section`,
  `constructor`, `destructor`, `visibility`, asm labels (`int x asm("y")`);
  others are accepted and ignored.
- Builtins: `__builtin_va_*`, `offsetof`, `expect`, `constant_p`, `choose_expr`,
  `types_compatible_p`, `classify_type`, `clz/ctz/popcount/parity/clrsb/bswap`,
  `add/sub/mul_overflow` (all variants), `frame_address`, `return_address`,
  `unreachable`, `trap`, float classification (`isnan`, `isinf`, …) and
  `__builtin_<libc function>`.
- Code that a constant condition makes unreachable (`if (0) f();`) is not
  emitted, so it may refer to functions that do not exist.

## Design

    src/lex.rs         tokenizer (shared by the preprocessor and the parser)
    src/preprocess.rs  macros (hideset algorithm), #if, #include, built-in headers
    src/types.rs       C type representation, ABI classification helpers
    src/parse.rs       recursive-descent parser -> typed AST, constant evaluation, initializers
    src/codegen.rs     AST -> x86-64 assembly (stack machine, System V calling convention)
    src/opt.rs         removal of unreachable code from the generated assembly
    src/main.rs        driver
    include/           built-in headers (stddef.h, stdarg.h, float.h, ...) and
                       builtins.h, the GNU builtins that can be written in C

The structure follows Rui Ueyama's [chibicc](https://github.com/rui314/chibicc):
every expression is evaluated into `%rax` / `%xmm0` / `%st(0)`, with the machine
stack for temporaries when an operand is not a constant or a variable.

`__GNUC__` is deliberately not defined: glibc's headers then avoid GNU-only
constructs.

## Not supported

- inline assembly with operands (`asm("..." : outputs : inputs)`); file-scope
  asm, asm without operands and the empty-template "barrier" form work
- `_Complex`, `_Atomic`, `__int128`, vector types, nested functions,
  `__builtin_setjmp`
- `const`/`volatile` are parsed and ignored: no diagnostics for writes to
  `const` objects, and `_Generic` cannot tell `char *` from `const char *`
  (nor `long` from `long long`)
- constant expressions of type `long double` in static initializers are
  folded in double precision (plain constants are exact)
- locals cannot be aligned to more than 16 bytes
- few diagnostics for invalid programs: rcc assumes its input compiles with
  gcc or clang, and stops at the first error
- no debug info, no real optimizer
