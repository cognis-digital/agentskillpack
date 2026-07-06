#!/usr/bin/env bash
# Build agentskillpack in release mode and install it onto your PATH.
#
# Usage:  ./install.sh [dest-dir]
# Default dest: $CARGO_HOME/bin (or ~/.cargo/bin), falling back to /usr/local/bin.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Install Rust from https://rustup.rs" >&2
  exit 1
fi

echo "building release binary..."
cargo build --release

bin="$here/target/release/agentskillpack"
[ -f "$bin" ] || bin="$here/target/release/agentskillpack.exe"

dest="${1:-}"
if [ -z "$dest" ]; then
  if [ -n "${CARGO_HOME:-}" ] && [ -d "$CARGO_HOME/bin" ]; then
    dest="$CARGO_HOME/bin"
  elif [ -d "$HOME/.cargo/bin" ]; then
    dest="$HOME/.cargo/bin"
  else
    dest="/usr/local/bin"
  fi
fi

mkdir -p "$dest"
cp "$bin" "$dest/"
echo "installed agentskillpack -> $dest"
echo "run: agentskillpack --help"
