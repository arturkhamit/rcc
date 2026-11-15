#!/bin/bash
# Builds real programs with rcc and checks them: Lua 5.4 (its own test suite),
# SQLite (same output as a gcc build), and projects built with their own build
# systems and checked by their own tests. Sources are expected in tests/ext.
cd "$(dirname "$0")/.." || exit 1
RCC=$PWD/target/release/rcc; EXT=$PWD/tests/ext
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
fail=0

if [ -d $EXT/lua-5.4.7 ]; then
  objs=()
  for f in $EXT/lua-5.4.7/src/*.c; do
    [ "$(basename $f)" = luac.c ] && continue
    o=$tmp/$(basename $f .c).o; objs+=($o)
    $RCC -DLUA_USE_LINUX -c $f -o $o || fail=1
  done
  $RCC "${objs[@]}" -o $tmp/lua -lm -ldl -Wl,-E || fail=1
  if (cd $EXT/lua-git/testes && $tmp/lua -e "_U=true" all.lua 2>&1 | grep -q 'final OK'); then echo "lua: OK"; else echo "lua: FAIL"; fail=1; fi
fi

S=$EXT/sqlite-amalgamation-3460000
if [ -d $S ]; then
  cat > $tmp/t.sql <<'SQL'
CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, score REAL, data BLOB);
WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<5000)
INSERT INTO t SELECT x, 'name' || (x*7919 % 1000), x * 1.5 / 7, randomblob(0) FROM c;
CREATE INDEX t_name ON t(name);
SELECT count(*), sum(id), round(avg(score), 6), min(name), max(name) FROM t;
SELECT name, count(*) c, printf('%.3f', sum(score)) FROM t GROUP BY name ORDER BY c DESC, name LIMIT 10;
SELECT a.id, b.id FROM t a JOIN t b ON a.name = b.name AND a.id < b.id ORDER BY a.id, b.id LIMIT 10;
UPDATE t SET score = score * 2 WHERE id % 3 = 0;
DELETE FROM t WHERE id % 7 = 0;
SELECT count(*), printf('%.4f', sum(score)), group_concat(id, ',') FILTER (WHERE id < 30) FROM t;
SELECT json_group_array(id) FROM t WHERE id < 10;
SELECT upper(hex(substr(name, 1, 3))), length(name), instr(name, '9'), 7.0/3, 1e300*1e10, -5 % 3, 10 / 4, abs(-9223372036854775807) FROM t LIMIT 3;
SELECT date('2024-02-29', '+1 year'), strftime('%Y-%j', '2000-12-31'), julianday('2000-01-01');
SELECT id, row_number() OVER (ORDER BY score DESC), rank() OVER (ORDER BY name) FROM t ORDER BY id LIMIT 5;
PRAGMA integrity_check;
SQL
  [ -x $S/sqlite_gcc ] || gcc -w -O1 $S/shell.c $S/sqlite3.c -o $S/sqlite_gcc -lpthread -ldl -lm
  $RCC $S/shell.c $S/sqlite3.c -o $tmp/sqlite -lpthread -ldl -lm 2>/dev/null || fail=1
  $tmp/sqlite :memory: < $tmp/t.sql > $tmp/rcc.out 2>&1
  $S/sqlite_gcc :memory: < $tmp/t.sql > $tmp/gcc.out 2>&1
  if cmp -s $tmp/rcc.out $tmp/gcc.out && grep -q '^ok$' $tmp/rcc.out; then echo "sqlite: OK ($(wc -l < $tmp/rcc.out) lines match gcc)"; else echo "sqlite: FAIL"; diff $tmp/rcc.out $tmp/gcc.out | head -10; fail=1; fi
fi

# Projects built with their own build systems (out of tree copies are not made:
# each is cleaned first). A project passes if its own test suite does.
check() { if [ $2 -eq 0 ]; then echo "$1: OK"; else echo "$1: FAIL"; fail=1; fi; }
if [ -d $EXT/bzip2/bzip2-1.0.8 ]; then
  (cd $EXT/bzip2/bzip2-1.0.8 && make clean && make CC=$RCC) > $tmp/bzip2.log 2>&1; check bzip2 $?
fi
if [ -d $EXT/zlib/zlib-1.3.1 ]; then
  (cd $EXT/zlib/zlib-1.3.1 && { make distclean; CC=$RCC CFLAGS="-O -fPIC" ./configure && make && make test; }) > $tmp/zlib.log 2>&1; check zlib $?
fi
if [ -d $EXT/chibicc ]; then
  (cd $EXT/chibicc && make clean && make CC=$RCC && make test CC=$RCC) > $tmp/chibicc.log 2>&1; check chibicc $?
fi
if [ -d $EXT/lz4 ]; then
  (cd $EXT/lz4 && make clean && make CC=$RCC -C tests test-lz4-essentials test-frametest test-fuzzer FUZZER_TIME=-T10s) > $tmp/lz4.log 2>&1; check lz4 $?
fi
if [ -d $EXT/zstd ]; then
  # The command-line tests, then the unit tests and five seconds of fuzzing.
  (cd $EXT/zstd && make clean && make CC=$RCC -C tests shortest && make CC=$RCC -C tests fuzzer && tests/fuzzer -T5s) > $tmp/zstd.log 2>&1; check zstd $?
fi
if [ -d $EXT/quickjs ]; then
  # Without Temporal and Intl: they need __int128.
  (cd $EXT/quickjs && make clean && make CC=$RCC CONFIG_LTO= CONFIG_TEMPORAL_INTL= all test) > $tmp/quickjs.log 2>&1; check quickjs $?
fi
if [ -d $EXT/tinycc ]; then
  # tcc built by rcc must compile and run its example programs and itself.
  (cd $EXT/tinycc && { make distclean; ./configure --cc=$RCC && make && make -C tests hello-exe hello-run libtest libtest_mt; }) > $tmp/tcc.log 2>&1; check tinycc $?
  # tcc's own feature test (without its inline assembly part), compiled by rcc and by gcc.
  sed 's/^#if (defined(__i386__) || defined(__x86_64__)) && !(defined _WIN32 && CC_NAME == CC_clang)$/#if 0/' $EXT/tinycc/tests/tcctest.c > $tmp/tcctest.c
  flags="-DCC_unknown=0 -I$EXT/tinycc -I$EXT/tinycc/tests -w"
  $RCC $tmp/tcctest.c -o $tmp/tcctest.rcc $flags 2>/dev/null && gcc $tmp/tcctest.c -o $tmp/tcctest.gcc $flags -O0 -std=gnu99 2>/dev/null \
    && (cd $EXT/tinycc/tests && $tmp/tcctest.rcc > $tmp/tt.rcc 2>&1; $tmp/tcctest.gcc > $tmp/tt.gcc 2>&1)
  # __builtin_constant_p of expressions only an optimizer folds is the one accepted difference.
  n=$(diff $tmp/tt.rcc $tmp/tt.gcc | grep -c '^[<>]' ); bad=$(diff $tmp/tt.rcc $tmp/tt.gcc | grep '^[<>]' | grep -vc 'res1[012] = ')
  [ -s $tmp/tt.rcc ] && [ "$bad" = 0 ]; check "tcctest ($(wc -l < $tmp/tt.rcc) lines, $n differ from gcc)" $?
fi
exit $fail
