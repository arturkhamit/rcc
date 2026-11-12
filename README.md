# rcc — a C compiler written in Rust

`rcc` compiles C11 (most of it) to x86-64 Linux assembly (System V ABI).
It has its own preprocessor, lexer, parser, type checker and code generator —
no dependencies besides the Rust standard library. The system `as` and `cc`
are used only to assemble and link.

## Build and use

    cargo build --release
    ./target/release/rcc hello.c -o hello && ./hello
    ./target/release/rcc -c a.c b.c          # object files
    ./target/release/rcc -S a.c              # assembly
    ./target/release/rcc -E a.c              # preprocess only

Options: `-o`, `-c`, `-S`, `-E`, `-I`, `-D`, `-U`, `-l`, `-L`; other flags
(`-O2`, `-g`, `-W...`, `-std=...`) are accepted and ignored.

## Status

| Test | Result |
|------|--------|
| [c-testsuite](https://github.com/c-testsuite/c-testsuite) single-exec | 216 / 220 |
| Local differential tests vs gcc (`tests/local`) | all pass |
| Lua 5.4.7 built with rcc, official test suite (`all.lua`) | passes |

Run the tests with `./fetch_tests.sh && ./run_tests.sh`.

## Design

    src/lex.rs         tokenizer (shared by the preprocessor and the parser)
    src/preprocess.rs  macros (hideset algorithm), #if, #include, built-in headers
    src/types.rs       C type representation
    src/parse.rs       recursive-descent parser -> typed AST, constant evaluation, initializers
    src/codegen.rs     AST -> x86-64 assembly (stack machine, System V calling convention)
    src/main.rs        driver
    include/           built-in headers (stddef.h, stdarg.h, stdbool.h, float.h, ...)

The structure follows Rui Ueyama's [chibicc](https://github.com/rui314/chibicc).
The generated code is unoptimized: every expression is evaluated into `%rax` /
`%xmm0` using the machine stack for temporaries.

## Not supported (yet)

- `long double` is treated as `double` (so `printf("%Lf")` and `strtold` do not work)
- variable-length arrays, `alloca`
- `_Generic`, `_Complex`, `_Atomic`, thread-local storage
- K&R-style function definitions, inline assembly, most GNU extensions
  (statement expressions, `typeof` and `__attribute__` parsing are supported)
- `const`/`volatile` are parsed and ignored; few diagnostics for invalid programs
- no optimization, no debug info
