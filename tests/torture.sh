#!/bin/bash
# Runs gcc.c-torture/execute against rcc. Usage: tests/torture.sh <dir-with-tests> [results-file]
RCC=${RCC:-$(cd "$(dirname "$0")/.." && pwd)/target/release/rcc}
dir=$1; res=${2:-/tmp/rcc-torture.txt}
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
run_one() {
  local c=$1 t; t=$tmp/$(basename "$c" .c)
  if ! timeout 10 "$RCC" -o "$t" "$c" >"$t.err" 2>&1; then
    echo "CE $(basename $c) $(grep -v 'warning:' "$t.err" | head -1 | sed -E 's/^[^ ]*:[0-9]+: //' | cut -c1-90)"; return; fi
  timeout 10 "$t" >/dev/null 2>&1; local rc=$?
  [ $rc -eq 0 ] && echo "OK $(basename $c)" || echo "RT $(basename $c) exit=$rc"
}
export -f run_one; export RCC tmp
ls "$dir"/*.c | xargs -P 10 -I{} bash -c 'run_one {}' | sort > "$res"
echo "passed $(grep -c '^OK' "$res")/$(wc -l < "$res"); compile errors $(grep -c '^CE' "$res"); wrong at runtime $(grep -c '^RT' "$res")"
echo "--- compile error categories"
grep '^CE' "$res" | cut -d' ' -f3- | sed -E "s/'[^']*'/'X'/g; s/[0-9]+/N/g" | sort | uniq -c | sort -rn | head -${TOP:-25}
