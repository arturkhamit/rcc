#!/bin/bash
# Speed of the code rcc generates, next to gcc: builds a few CPU-bound programs
# from tests/ext with each compiler configuration and times the same workload.
# Usage: tests/bench.sh [config...]     default: "rcc" "rcc -O1" "gcc -O0" "gcc -O2"
#        BENCH=lua tests/bench.sh       only the programs whose name starts so
cd "$(dirname "$0")/.." || exit 1
RCC=$PWD/target/release/rcc; EXT=$PWD/tests/ext
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
[ $# -eq 0 ] && set -- "rcc" "rcc -O1" "gcc -O0" "gcc -O2"
DATA=$EXT/sqlite-amalgamation-3460000/sqlite3.c # 9 MB of text to compress

cat > $tmp/bench.lua <<'LUA'
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local t, s = {}, 0
for i = 1, 3000000 do t[i] = (i * 7919) % 1000003 end
table.sort(t)
for i = 1, #t do s = (s + t[i] * i) % 1000000007 end
local parts = {}
for i = 1, 200000 do parts[#parts + 1] = string.format("%d:%s", i, tostring(i * 3.5)) end
print(fib(32), s, #table.concat(parts))
LUA
cat > $tmp/bench.sql <<'SQL'
CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, score REAL);
WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<400000)
INSERT INTO t SELECT x, 'name' || (x*7919 % 5000), x * 1.5 / 7 FROM c;
CREATE INDEX t_name ON t(name);
SELECT count(*), sum(id), round(avg(score), 3) FROM t;
SELECT name, count(*) c, round(sum(score), 2) FROM t GROUP BY name ORDER BY c DESC, name LIMIT 3;
SELECT count(*) FROM t a JOIN t b ON a.name = b.name AND a.id < b.id WHERE a.id < 2000;
UPDATE t SET score = score * 2 WHERE id % 3 = 0;
SELECT round(sum(score), 2) FROM t WHERE name LIKE '%99%';
SQL

# name | sources (relative to tests/ext) | compile flags | link flags | workload (the binary is $exe)
progs=(
  "bzip2|bzip2/bzip2-1.0.8/{blocksort,huffman,crctable,randtable,compress,decompress,bzlib,bzip2}.c|||\$exe -k -c $DATA"
  "lz4 -9|lz4/lib/*.c lz4/programs/*.c|-Ilz4/lib -DLZ4IO_MULTITHREAD=0||\$exe -9 -c $DATA"
  "zstd -9|zstd/lib/common/*.c zstd/lib/compress/*.c zstd/lib/decompress/*.c zstd/programs/{zstdcli,util,timefn,fileio,fileio_asyncio,benchfn,benchzstd,datagen,dibio,zstdcli_trace,lorem}.c zstd/lib/dictBuilder/*.c|-Izstd/lib -Izstd/lib/common -Izstd/lib/dictBuilder -DZSTD_MULTITHREAD=0 -DZSTD_DISABLE_ASM|-lpthread|\$exe -9 -T1 -c $DATA"
  "lua|lua-5.4.7/src/{lapi,lauxlib,lbaselib,lcode,lcorolib,lctype,ldblib,ldebug,ldo,ldump,lfunc,lgc,linit,liolib,llex,lmathlib,lmem,loadlib,lobject,lopcodes,loslib,lparser,lstate,lstring,lstrlib,ltable,ltablib,ltm,lundump,lutf8lib,lvm,lzio,lua}.c|-DLUA_USE_LINUX|-lm -ldl|\$exe $tmp/bench.lua"
  "sqlite|sqlite-amalgamation-3460000/{shell,sqlite3}.c||-lpthread -ldl -lm|\$exe :memory: < $tmp/bench.sql"
)

printf '%-10s' ""; for cfg in "$@"; do printf '%12s' "$cfg"; done; echo
for p in "${progs[@]}"; do
  IFS='|' read -r name srcs cflags ldflags work <<< "$p"
  [[ $name == ${BENCH:-}* ]] || continue
  printf '%-10s' "$name"
  ref=""
  for cfg in "$@"; do
    cc=${cfg/#rcc/$RCC}; exe=$tmp/bin; rm -f $exe
    if ! (cd $EXT && eval "$cc -w $cflags $srcs -o $exe $ldflags") > $tmp/build.log 2>&1; then printf '%12s' "build fails"; continue; fi
    # Best of three runs; the output must be the same for every configuration.
    best=""
    for _ in 1 2 3; do
      s=$(date +%s.%N); eval "$work" > $tmp/out 2>/dev/null; rc=$?; e=$(date +%s.%N)
      t=$(echo "$e - $s" | bc); [ -z "$best" ] || [ "$(echo "$t < $best" | bc)" = 1 ] && best=$t
    done
    sum=$(md5sum < $tmp/out); [ -z "$ref" ] && ref=$sum
    if [ $rc -ne 0 ] || [ "$sum" != "$ref" ]; then printf '%12s' "WRONG"; else printf '%11.2fs' "$best"; fi
  done
  echo
done
