#!/bin/bash
# End to end: the real decisions binary in a pseudo-terminal against the real bin/decide,
# in a sandbox state dir per case. Builds the binary first. Asserts the inbox messages
# decide writes, which are what the /run-day session reads.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE="$(dirname "$HERE")"
REPO="$(dirname "$CRATE")"
cargo build --quiet --manifest-path "$CRATE/Cargo.toml" || exit 1
BIN="$CRATE/target/debug/decisions"
export DECIDE="$REPO/bin/decide"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  ok   - $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL - $1"; echo "         $2"; }
assert_eq() { [ "$1" = "$2" ] && pass "$3" || fail "$3" "expected [$1] got [$2]"; }
assert_contains() { case "$1" in *"$2"*) pass "$3" ;; *) fail "$3" "[$2] not in [$1]" ;; esac; }

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

# A fresh state dir with two open decisions: D-1 (Yes ★ / No) and D-2.
fresh() {
  export RUN_DAY_STATE_DIR="$SANDBOX/$1" INBOX_DIR="$SANDBOX/$1/inbox"
  mkdir -p "$INBOX_DIR"
  echo "ctx" | "$DECIDE" add --key ACME-1 --question "First?" --option "Yes" --option "No" --recommend 1 >/dev/null
  echo "ctx" | "$DECIDE" add --key ACME-2 --question "Second?" --option "Yes" --option "No" >/dev/null
}
inbox() { cat "$INBOX_DIR"/*.md 2>/dev/null; }
drive() { python3 "$HERE/drive.py" "$BIN" "$@"; }

echo "decisions — end to end"

fresh wait
rc=$(drive 1 sleep:6 q)
assert_eq 0 "$rc" "exits 0"
assert_contains "$(inbox)" "D-1 answered: 1 — Yes" "an answer is sent when the undo window runs out"
assert_eq 1 "$("$DECIDE" count)" "one decision left open"

fresh undo
drive 1 u q >/dev/null
assert_eq "" "$(inbox)" "undo sends nothing"
assert_eq 2 "$("$DECIDE" count)" "both decisions still open"

fresh note
drive 2 n "use staging" $'\r' q >/dev/null
assert_contains "$(inbox)" "D-1 answered: 2 — No. Note: use staging" "a note goes with the answer"

fresh tell
drive 3 "close it as a duplicate" $'\r' q >/dev/null
assert_contains "$(inbox)" "D-1 answered in your own words: close it as a duplicate." "the last slot answers in your own words"

fresh cardnote
drive n "use staging" $'\r' 1 q >/dev/null
assert_contains "$(inbox)" "D-1 answered: 1 — Yes. Note: use staging." "a note written before answering goes with the answer"

fresh ask
drive a "which one?" $'\r' q >/dev/null
assert_contains "$(inbox)" "D-1: the user asks: which one?" "ask back reaches the session"
assert_eq 2 "$("$DECIDE" count)" "asking leaves the decision open"

fresh quit
drive 1 q >/dev/null
assert_contains "$(inbox)" "D-1 answered: 1 — Yes" "quitting sends the pending answer"

fresh term
drive 1 signal:TERM >/dev/null
assert_contains "$(inbox)" "D-1 answered: 1 — Yes" "SIGTERM with an answer pending sends it"

fresh hup
drive 1 signal:HUP >/dev/null
assert_contains "$(inbox)" "D-1 answered: 1 — Yes" "SIGHUP with an answer pending sends it"

fresh demo
rc=$(DRIVE_ARGS=--demo DECIDE=/nonexistent/decide drive 1 sleep:6 2 q)
assert_eq 0 "$rc" "--demo runs without decide and quits cleanly"
assert_eq "" "$(inbox)" "--demo answers never reach the inbox"
assert_eq 2 "$("$DECIDE" count)" "--demo leaves the real queue alone"

fresh nodecide
rc=$(DECIDE=/nonexistent/decide drive q)
assert_eq 0 "$rc" "a missing decide still opens and quits cleanly"

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
