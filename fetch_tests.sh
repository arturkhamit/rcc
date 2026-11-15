#!/bin/bash
# Downloads the test corpora (none of them is committed to this repository).
#   ./fetch_tests.sh       c-testsuite, used by run_tests.sh
#   ./fetch_tests.sh all   also the gcc torture tests (tests/torture.sh) and the
#                          real programs built by tests/real.sh
cd "$(dirname "$0")/tests" || exit 1
[ -d c-testsuite ] || git clone --depth 1 https://github.com/c-testsuite/c-testsuite
[ "$1" = all ] || exit 0
mkdir -p ext && cd ext || exit 1
get() { [ -d "$2" ] || { mkdir -p "$2" && curl -sL "$1" | tar -xz -C "$2" --strip-components=1; }; }
get https://www.lua.org/ftp/lua-5.4.7.tar.gz lua-5.4.7
[ -d lua-git ] || git clone -q --depth 1 --branch v5.4.7 https://github.com/lua/lua lua-git
[ -d sqlite-amalgamation-3460000 ] || { curl -sLO https://www.sqlite.org/2024/sqlite-amalgamation-3460000.zip && unzip -q sqlite-amalgamation-3460000.zip; }
get https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz zlib/zlib-1.3.1
get https://sourceware.org/pub/bzip2/bzip2-1.0.8.tar.gz bzip2/bzip2-1.0.8
[ -d chibicc ] || git clone -q --depth 1 https://github.com/rui314/chibicc
[ -d tinycc ] || git clone -q --depth 1 https://github.com/TinyCC/tinycc
[ -d lz4 ] || git clone -q --depth 1 --branch v1.10.0 https://github.com/lz4/lz4
[ -d zstd ] || git clone -q --depth 1 --branch v1.5.6 https://github.com/facebook/zstd
[ -d quickjs ] || git clone -q --depth 1 https://github.com/bellard/quickjs
if [ ! -d gcc-ts ]; then
  git clone -q --depth 1 --filter=blob:none --sparse https://github.com/gcc-mirror/gcc gcc-ts
  git -C gcc-ts sparse-checkout set gcc/testsuite/gcc.c-torture/execute
fi
