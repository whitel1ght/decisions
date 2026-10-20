#!/bin/bash
# The decisions CLI against a sandbox store: every command, exit code and event.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE="$(dirname "$HERE")"
cargo build --quiet --manifest-path "$CRATE/Cargo.toml" || exit 1
BIN="$CRATE/target/debug/decisions"
PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "  ok   - $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL - $1"; echo "         $2"; }
check() { [ "$2" = "$3" ] && pass "$1" || fail "$1" "want [$2], got [$3]"; }
has() { case "$2" in *"$3"*) pass "$1" ;; *) fail "$1" "[$3] not in: $2" ;; esac; }

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX/home" DECISIONS_DIR="$SANDBOX/store" DECISIONS_MR_STRIP=acme-
export DECISIONS_ON_ANSWER="cat >> '$SANDBOX/events'"
D="$DECISIONS_DIR"
run() { "$BIN" "$@" 2>&1; }
events() { cat "$SANDBOX/events" 2>/dev/null; }

echo "add"
id="$(printf '@alex: "rename to trackCount" (nit)\nPipeline green.\n' | run add --key acme-18313 \
  --mr https://gitlab.com/acme/acme-admin/-/merge_requests/315 --blocks merge \
  --question 'Fix the nit, or merge now?' --option 'Fix now, then merge' --option 'Merge, follow-up ticket' \
  --option 'Decline: the name stays' --recommend 1)"
check "prints the new id" "D-1" "$id"
f="$D/D-1.md"
has "frontmatter in field order" "$(sed -n 2,8p "$f" | cut -d: -f1 | tr '\n' ' ')" "id day key mr asked blocks status"
has "key upper-cased" "$(cat "$f")" "key: ACME-18313"
has "options numbered, recommended marked" "$(cat "$f")" "1. Fix now, then merge (recommended)"
id2="$(run add --key '' --source 'codex · repo' --question 'Weird: text | "quotes" ✓' --option a --option b </dev/null)"
check "second id" "D-2" "$id2"
check "source kept" "codex · repo" "$(run list --json | jq -r '.[] | select(.id=="D-2") | .source')"

echo "refusals and usage"
run add --key X-1 --question q --option only </dev/null >/dev/null; check "one option refused" "1" "$?"
run add --key 'not a key' --question q --option a --option b </dev/null >/dev/null; check "bad key refused" "1" "$?"
out="$(run add --key ACME-18313 --question 'Fix the nit, or merge now?' --option a --option b </dev/null)"; code=$?
check "duplicate refused" "1" "$code"; has "names the existing one" "$out" "D-1"
run answer D-1 >/dev/null; check "missing argument is a usage error" "2" "$?"

echo "list, show, count"
out="$(run list)"
check "two open" "2" "$(printf '%s\n' "$out" | wc -l | tr -d ' ')"
has "blocking first with its MR" "$(printf '%s\n' "$out" | head -1)" "admin!315"
check "json mr_short" "admin!315" "$(run list --json | jq -r '.[] | select(.id=="D-1") | .mr_short')"
check "json age" "0m" "$(run list --json | jq -r '.[] | select(.id=="D-1") | .age')"
printf 'garbage\n' > "$D/D-9.md"
"$BIN" list >/dev/null 2>"$SANDBOX/err"; has "a bad file is named on stderr" "$(cat "$SANDBOX/err")" "D-9.md"
rm "$D/D-9.md"
has "show prints the file" "$(run show D-1)" "## Question"
run show D-77 >/dev/null; check "show of a missing id exits 1" "1" "$?"
check "count" "2" "$(run count)"; check "badge" "✋ 2" "$(run count --badge)"

echo "ask, reply, answer, tell and their events"
run ask D-1 'is it safe?' >/dev/null; check "ask exits 0" "0" "$?"
has "asked event" "$(events | tail -1)" '"text":"D-1: the user asks: is it safe?"'
"$BIN" wait D-1 --timeout 1 >"$SANDBOX/out"; code=$?
check "wait sees the ask-back" "5" "$code"; has "and says it" "$(cat "$SANDBOX/out")" "the user asked back: is it safe?"
run reply D-1 'yes, green' >/dev/null
"$BIN" wait D-1 --timeout 1 >"$SANDBOX/out"; check "pending after the reply" "3" "$?"
has "pending text" "$(cat "$SANDBOX/out")" "pending: D-1 is still open"
( sleep 1; "$BIN" answer D-1 1 'ship it: today' >/dev/null ) &
"$BIN" wait D-1 --timeout 10 >"$SANDBOX/out"; check "wait returns the answer" "0" "$?"
has "answered text" "$(cat "$SANDBOX/out")" "answered: 1 — Fix now, then merge"
has "answered event" "$(events | tail -1)" '"text":"D-1 answered: 1 — Fix now, then merge. Note: ship it: today."'
run answer D-1 2 >/dev/null; check "answering twice exits 1" "1" "$?"
run tell D-2 'close it as a duplicate' >/dev/null
has "told event" "$(events | tail -1)" '"event":"told"'
"$BIN" wait D-2 >"$SANDBOX/out"; check "wait on an own-words answer" "0" "$?"
has "own words text" "$(cat "$SANDBOX/out")" "answered in your own words: close it as a duplicate"
DECISIONS_ON_ANSWER='exit 9' "$BIN" add --key '' --question 'Hooked?' --option a --option b </dev/null >/dev/null
out="$(DECISIONS_ON_ANSWER='echo broken >&2; exit 9' "$BIN" answer D-3 1 2>&1)"; code=$?
check "a failing hook still answers" "0" "$code"; has "and warns" "$out" "warning: on_answer failed"
check "the answer stands" "answered: 1 — a" "$("$BIN" wait D-3)"

echo "withdraw, close, numbering"
wid="$(run add --key ACME-2 --question 'Defer?' --option yes --option no </dev/null)"
run close "$wid" >/dev/null; check "closing an open one exits 1" "1" "$?"
run withdraw "$wid" reviewer resolved it >/dev/null
"$BIN" wait "$wid" >"$SANDBOX/out"; check "wait on a withdrawn one" "4" "$?"
has "withdrawn text" "$(cat "$SANDBOX/out")" "withdrawn: $wid (reviewer resolved it)"
run close D-1 >/dev/null; run close "$wid" >/dev/null
[ -f "$D/done/$(date +%F)/D-1.md" ] && pass "closed ones move to done/<day>" || fail "closed ones move to done/<day>" "$(find "$D" -name 'D-*')"
check "wait finds a closed one" "0" "$("$BIN" wait D-1 >/dev/null; echo $?)"
check "numbering continues past done/" "D-$(( ${wid#D-} + 1 ))" "$(run add --key ACME-3 --question 'Next?' --option a --option b </dev/null)"

echo "setup"
for h in claude codex gemini opencode pi cursor generic; do
  has "setup $h names the server command" "$(run setup $h)" "decisions mcp"
done
run setup nonsense >/dev/null; check "unknown harness exits 2" "2" "$?"

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
