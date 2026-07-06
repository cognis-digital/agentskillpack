#!/usr/bin/env bash
# Run every agentskillpack demo end to end. Exits 0 only if all demos pass.
#
# Usage:  demos/run_all.sh
# Requires: a release or debug build of the CLI (this script builds it).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

echo "== building agentskillpack =="
cargo build --quiet
BIN="$root/target/debug/agentskillpack"
[ -f "$BIN" ] || BIN="$root/target/debug/agentskillpack.exe"

# A private scratch workspace, cleaned on exit.
WORK="$(mktemp -d 2>/dev/null || echo "${TMPDIR:-/tmp}/asp_demo_$$")"
mkdir -p "$WORK"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

pass=0
fail=0
demo() {
  local name="$1"; shift
  echo
  echo "== demo: $name =="
  if "$@"; then
    echo "-- $name: PASS"
    pass=$((pass + 1))
  else
    echo "-- $name: FAIL"
    fail=$((fail + 1))
  fi
}

# ---------------------------------------------------------------------------
# Demo 1: end-to-end lifecycle
#   pack -> sign -> verify -> registry add -> resolve -> lock -> unpack
# ---------------------------------------------------------------------------
demo_lifecycle() {
  local arc="$WORK/hello.skillpack"
  "$BIN" pack examples/hello-skill -o "$arc" --validate
  "$BIN" info "$arc"

  "$BIN" keygen -o "$WORK/keys" --name author
  "$BIN" sign "$arc" --key "$WORK/keys/author.key"
  "$BIN" verify "$arc" --pubkey "$WORK/keys/author.pub"

  local reg="$WORK/registry"
  "$BIN" registry add "$arc" --registry "$reg"
  "$BIN" registry list --registry "$reg"
  "$BIN" registry resolve hello-skill --req '^1.0' --registry "$reg"

  # Pack the richer skill (depends on hello-skill), then lock against the reg.
  local rarc="$WORK/research.skillpack"
  "$BIN" pack examples/research-skill -o "$rarc" --validate
  "$BIN" registry add "$rarc" --registry "$reg"
  "$BIN" lock examples/research-skill --registry "$reg" -o "$WORK/skillpack.lock"
  cat "$WORK/skillpack.lock"

  "$BIN" unpack "$arc" -o "$WORK/restored"
  test -f "$WORK/restored/scripts/greet.py"
}

# ---------------------------------------------------------------------------
# Demo 2: tamper detection — a flipped byte must fail verify (nonzero).
# ---------------------------------------------------------------------------
flip_last_byte() {
  # Portably flip the value of the byte 3 positions from the end of $1.
  local f="$1"
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$f" <<'PY'
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
b[-3] ^= 0xFF
open(p, "wb").write(b)
PY
  else
    # Fallback: rewrite the whole file with one byte XOR'd, using printf/od.
    local size off
    size=$(wc -c < "$f")
    off=$((size - 3))
    local byte
    byte=$(od -An -tu1 -j "$off" -N1 "$f" | tr -d ' ')
    byte=$(( byte ^ 0xFF ))
    printf "$(printf '\\%03o' "$byte")" | dd of="$f" bs=1 seek="$off" conv=notrunc count=1 2>/dev/null
  fi
}

demo_tamper() {
  local arc="$WORK/tamper.skillpack"
  "$BIN" pack examples/hello-skill -o "$arc"
  # Corrupt one byte near the end (a file blob).
  flip_last_byte "$arc"
  if "$BIN" verify "$arc"; then
    echo "ERROR: tampered archive verified (should not)"; return 1
  fi
  echo "tamper correctly rejected (nonzero exit)"
}

# ---------------------------------------------------------------------------
# Demo 3: wrong-key rejection — verifying with the wrong pubkey must fail.
# ---------------------------------------------------------------------------
demo_wrong_key() {
  local arc="$WORK/wk.skillpack"
  "$BIN" pack examples/hello-skill -o "$arc"
  "$BIN" keygen -o "$WORK/k1" --name signer
  "$BIN" keygen -o "$WORK/k2" --name other
  "$BIN" sign "$arc" --key "$WORK/k1/signer.key"
  if "$BIN" verify "$arc" --pubkey "$WORK/k2/other.pub"; then
    echo "ERROR: wrong key accepted (should not)"; return 1
  fi
  echo "wrong key correctly rejected (nonzero exit)"
}

# ---------------------------------------------------------------------------
# Demo 4: manifest validation — a bad manifest must be rejected.
# ---------------------------------------------------------------------------
demo_manifest() {
  "$BIN" manifest validate examples/research-skill
  local bad="$WORK/bad"
  mkdir -p "$bad"
  printf '{"name":"Bad Name","version":"nope"}' > "$bad/skill.json"
  if "$BIN" manifest validate "$bad"; then
    echo "ERROR: invalid manifest passed (should not)"; return 1
  fi
  echo "invalid manifest correctly rejected"
}

demo "lifecycle (pack->sign->verify->registry->resolve->lock->unpack)" demo_lifecycle
demo "tamper detection" demo_tamper
demo "wrong-key rejection" demo_wrong_key
demo "manifest validation" demo_manifest

echo
echo "==================================="
echo "demos passed: $pass  failed: $fail"
echo "==================================="
[ "$fail" -eq 0 ]
