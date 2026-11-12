#!/bin/bash
# Runs c-testsuite (tests/c-testsuite) and local tests (tests/local) against rcc.
# Usage: ./run_tests.sh [-v]   (-v: print the first error line of each failure)
RCC=${RCC:-$(dirname "$0")/target/release/rcc}
cd "$(dirname "$0")"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
run_one() {
  local c=$1 t; t=$tmp/$(basename "$c" .c)
  if ! timeout 10 "$RCC" -o "$t" "$c" >"$t.cerr" 2>&1; then echo "FAIL(compile) $c: $(grep -v warning "$t.cerr" | head -1)"; return; fi
  timeout 10 "$t" >"$t.out" 2>&1; local rc=$?
  if [ -f "$c.expected" ]; then
    if [ $rc -ne 0 ]; then echo "FAIL(exit $rc) $c"; return; fi
    cmp -s "$t.out" "$c.expected" || { echo "FAIL(output) $c"; return; }
  else # local tests: compare with gcc
    gcc -w -o "$t.ref" "$c" 2>/dev/null && timeout 10 "$t.ref" >"$t.refout" 2>&1; local rrc=$?
    [ $rc -eq $rrc ] && cmp -s "$t.out" "$t.refout" || { echo "FAIL(differs from gcc, exit $rc vs $rrc) $c"; return; }
  fi
  echo "ok $c"
}
export -f run_one; export RCC tmp
ls tests/c-testsuite/tests/single-exec/*.c tests/local/*.c 2>/dev/null | xargs -P 8 -I{} bash -c 'run_one {}' | sort > "$tmp/res"
# ABI: objects compiled by rcc and by gcc must call each other (tests/interop).
I=tests/interop
if gcc -std=gnu17 -w -c $I/callee.c -o $tmp/callee.g.o && gcc -std=gnu17 -w -c $I/caller.c -o $tmp/caller.g.o && gcc $tmp/callee.g.o $tmp/caller.g.o -o $tmp/interop && $tmp/interop > $tmp/interop.exp; then
  "$RCC" -c $I/callee.c -o $tmp/callee.r.o 2>$tmp/interop.err; "$RCC" -c $I/caller.c -o $tmp/caller.r.o 2>>$tmp/interop.err
  for pair in "callee.r caller.g" "callee.g caller.r" "callee.r caller.r"; do
    set -- $pair
    if gcc $tmp/$1.o $tmp/$2.o -o $tmp/interop 2>>$tmp/interop.err && $tmp/interop 2>&1 | cmp -s - $tmp/interop.exp; then
      echo "ok interop $pair" >> "$tmp/res"
    else
      echo "FAIL(interop) $pair $(head -1 $tmp/interop.err)" >> "$tmp/res"
    fi
  done
fi
grep -c '^ok' "$tmp/res" | xargs -I{} echo "passed {}/$(wc -l < "$tmp/res")"
grep -v '^ok' "$tmp/res" | sed 's|tests/c-testsuite/tests/single-exec/||' | { [ "$1" = -v ] && cat || cut -c1-110; } | head -${MAXFAIL:-40}
