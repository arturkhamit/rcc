#!/bin/bash
# Downloads c-testsuite into tests/c-testsuite (not committed to this repository).
cd "$(dirname "$0")/tests" && [ -d c-testsuite ] || git clone --depth 1 https://github.com/c-testsuite/c-testsuite
