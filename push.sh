#!/bin/bash
# Publishes rcc to GitHub.
#
# Uncommitted changes are committed first, one commit per part of the project
# (lexer, preprocessor, parser, codegen, ...), each with its own message. Then
# the branch is pushed. Commits that already exist are pushed as they are.
#
# Usage: ./push.sh [remote-url]
#   default remote: origin, or git@github.com:arturkhamit/rcc.git if there is none
set -euo pipefail
cd "$(dirname "$0")"
url=${1:-}
branch=$(git branch --show-current)

# Refuse to publish something that does not build.
cargo=$(command -v cargo || command -v cargo-1.93 || true)
if [ -n "$cargo" ]; then
  "$cargo" build --release -q || { echo "push.sh: the build fails, nothing pushed" >&2; exit 1; }
fi

# part | paths (most specific first: a path is taken by the first part naming it) | what the part is
parts=(
  "builtins|include/builtins.h|GNU builtins written in C (bit counting, overflow checks, float classification)"
  "lexer|src/lex.rs|tokenizer for identifiers, numbers, string and character literals"
  "preprocessor|src/preprocess.rs include|macro expansion, #if, #include, built-in headers"
  "types|src/types.rs|C type representation and ABI classification"
  "parser|src/parse.rs|declarations, statements, expressions, initializers, constant evaluation"
  "codegen|src/codegen.rs src/opt.rs|x86-64 code generation and removal of unreachable code"
  "driver|src/main.rs Cargo.toml Cargo.lock|command-line driver: compiling, assembling, linking"
  "tests|tests run_tests.sh fetch_tests.sh|tests and test scripts"
  "docs|README.md|documentation"
  "repo|.|repository files"
)

git reset -q # start from an empty index: whatever was staged is sorted by part too
for p in "${parts[@]}"; do
  IFS='|' read -r name paths about <<< "$p"
  for path in $paths; do
    git add -A -- "$path" 2>/dev/null || true # a part may have no files yet
  done
  git diff --cached --quiet && continue
  files=$(git diff --cached --name-only | xargs -n1 basename | sort -u | paste -sd, - | sed 's/,/, /g')
  stat=$(git diff --cached --shortstat | sed 's/ changed//; s/^ *//')
  git commit -q -F - <<EOF
$name: update $files

Part: $about.
$stat
EOF
  echo "committed: $(git log -1 --format=%s)"
done

if ! git remote get-url origin >/dev/null 2>&1; then
  git remote add origin "${url:-git@github.com:arturkhamit/rcc.git}"
elif [ -n "$url" ]; then
  git remote set-url origin "$url"
fi

if ! git ls-remote origin >/dev/null 2>&1; then
  repo=$(git remote get-url origin | sed -E 's#^(git@github.com:|https://github.com/)##; s#\.git$##')
  if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
    echo "creating the GitHub repository $repo (private)"
    gh repo create "$repo" --private --description "A C11 compiler for x86-64 Linux, written in Rust"
  else
    echo "push.sh: cannot reach $(git remote get-url origin)." >&2
    echo "Create an empty repository named '${repo#*/}' at https://github.com/new (no README)," >&2
    echo "make sure 'ssh -T git@github.com' greets you, then run ./push.sh again." >&2
    exit 1
  fi
fi

git push -u origin "$branch"
echo "pushed $branch: $(git rev-list --count HEAD) commits, $(git remote get-url origin)"
