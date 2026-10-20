#!/bin/bash
# End to end: the real decisions binary in a pseudo-terminal, on a sandbox store per case.
# Builds the binary first. Asserts the on_answer events, which are what an asker's tooling
# (for example a session's inbox) receives, and the store's state.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE="$(dirname "$HERE")"
cargo build --quiet --manifest-path "$CRATE/Cargo.toml" || exit 1
BIN="$CRATE/target/debug/decisions"

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  ok   - $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL - $1"; echo "         $2"; }
assert_eq() { [ "$1" = "$2" ] && pass "$3" || fail "$3" "expected [$1] got [$2]"; }
assert_contains() { case "$1" in *"$2"*) pass "$3" ;; *) fail "$3" "[$2] not in [$1]" ;; esac; }

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

# A fresh store with two open decisions: D-1 (Yes ★ / No) and D-2; "empty" leaves it empty.
fresh() {
  export DECISIONS_DIR="$SANDBOX/$1" HOME="$SANDBOX/home"
  export DECISIONS_ON_ANSWER="cat >> '$SANDBOX/$1.events'"
  mkdir -p "$DECISIONS_DIR"
  [ "${2:-}" = empty ] && return
  echo "ctx" | "$BIN" add --key ACME-1 --question "First?" --option "Yes" --option "No" --recommend 1 >/dev/null
  echo "ctx" | "$BIN" add --key ACME-2 --question "Second?" --option "Yes" --option "No" >/dev/null
}
inbox() { cat "$SANDBOX/$(basename "$DECISIONS_DIR").events" 2>/dev/null; }
drive() { python3 "$HERE/drive.py" "$BIN" "$@"; }

echo "decisions — end to end"

fresh wait
rc=$(drive 1 sleep:6 q)
assert_eq 0 "$rc" "exits 0"
assert_contains "$(inbox)" "D-1 answered: 1 — Yes" "an answer is sent when the undo window runs out"
assert_eq 1 "$("$BIN" count)" "one decision left open"

fresh undo
drive 1 u q >/dev/null
assert_eq "" "$(inbox)" "undo sends nothing"
assert_eq 2 "$("$BIN" count)" "both decisions still open"

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
assert_eq 2 "$("$BIN" count)" "asking leaves the decision open"

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
rc=$(DRIVE_ARGS=--demo drive 1 sleep:6 2 q)
assert_eq 0 "$rc" "--demo runs without decide and quits cleanly"
assert_eq "" "$(inbox)" "--demo answers never reach the inbox"
assert_eq 2 "$("$BIN" count)" "--demo leaves the real queue alone"

fresh mcp empty
DECISIONS_WAIT=0 python3 "$HERE/mcp_client.py" "$BIN" ask_user '{"question":"Ship the MCP build?","options":["Ship it","Hold"],"recommended":1}' > "$SANDBOX/mcp.out" &
client=$!
for _ in $(seq 1 50); do [ "$("$BIN" count)" = 1 ] && break; sleep 0.1; done
drive 1 sleep:6 q >/dev/null
wait "$client"
assert_contains "$(cat "$SANDBOX/mcp.out")" "answered: 1 — Ship it" "an MCP ask_user answered in the deck returns that answer"
assert_contains "$("$BIN" list --json; cat "$DECISIONS_DIR"/D-1.md)" "source: \"e2e-client · " "the card knows which client asked"

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
