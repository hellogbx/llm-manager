#!/usr/bin/env bash
# Build llmmgr. Uses the bundled Rust toolchain in ./.rust when present,
# otherwise whatever cargo happens to be on PATH.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -x "$ROOT/.rust/bin/cargo" ]; then
  export PATH="$ROOT/.rust/bin:$PATH"
  export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo}"
fi
cd "$ROOT"
cargo build "$@"
profile=debug
for arg in "$@"; do
  [ "$arg" = "--release" ] && profile=release
done
echo "binary: $ROOT/target/$profile/llmmgr"
