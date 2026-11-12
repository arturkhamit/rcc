#!/bin/bash
# Builds real programs with rcc and checks them: Lua 5.4 (its own test suite)
# and SQLite (same output as a gcc build). Sources are expected in tests/ext.
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
exit $fail
