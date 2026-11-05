#!/usr/bin/env bash
# The same checks as CI (format, clippy, tests), with the output saved to
# results/quality.txt. Tests run in release mode here. The absolute path of
# the checkout is replaced by `.` so the file does not depend on where the
# repository lives.
set -euo pipefail
cd "$(dirname "$0")/.."

{
    echo "\$ cargo fmt --all -- --check"
    cargo fmt --all -- --check 2>&1 && echo "ok"
    echo
    echo "\$ cargo clippy --all-targets -- -D warnings"
    cargo clippy --all-targets -- -D warnings 2>&1
    echo
    echo "\$ cargo test --release"
    cargo test --release 2>&1
} | sed "s|$PWD|.|g" > results/quality.txt
grep -E "^test result|^ok$|warning|error" results/quality.txt
