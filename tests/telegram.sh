#!/bin/bash
# End to end: the real binary against a fake Bot API (tests/fake_telegram.py) on a sandbox
# store: pair, send a card, answer it with a button, and check the store, the hook and the
# edited message.
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
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null; wait "$p" 2>/dev/null; done
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

python3 "$HERE/fake_telegram.py" >"$SANDBOX/port" &
PIDS+=($!)
for _ in $(seq 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
API="http://127.0.0.1:$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home" DECISIONS_DIR="$SANDBOX/store" DECISIONS_TELEGRAM_API="$API"
export DECISIONS_TELEGRAM_TOKEN=T1 DECISIONS_TELEGRAM_IDLE=0
export DECISIONS_ON_ANSWER="cat >> '$SANDBOX/events'"
unset DECISIONS_TELEGRAM_TOKEN_CMD
mkdir -p "$DECISIONS_DIR"

calls() { curl -s "$API/control/calls"; }
update() { curl -s -X POST -d "$1" "$API/control/update" >/dev/null; }
# Wait up to $2 seconds until the Python expression $1 holds for the call list c.
until_calls() {
  for _ in $(seq $(($2 * 10))); do
    calls | python3 -c "import json,sys; c=json.load(sys.stdin); sys.exit(0 if ($1) else 1)" && return 0
    sleep 0.1
  done
  return 1
}
state() { python3 -c "import json; print(json.load(open('$DECISIONS_DIR/telegram.json'))$1)"; }

echo "decisions telegram — end to end"

"$BIN" telegram run 2>/dev/null
assert_eq 1 $? "run refuses before pairing"
assert_contains "$("$BIN" telegram status)" "not paired" "status says not paired"

"$BIN" telegram pair >"$SANDBOX/pair.out" 2>&1 &
PAIR=$!
PIDS+=($PAIR)
for _ in $(seq 50); do grep -q "/start" "$SANDBOX/pair.out" && break; sleep 0.1; done
CODE=$(sed -n 's/.*\/start \([0-9]*\).*/\1/p' "$SANDBOX/pair.out")
assert_contains "$(cat "$SANDBOX/pair.out")" "@fake_bot" "pair names the bot"
update '{"message":{"message_id":1,"chat":{"id":99,"type":"group"},"text":"/start '"$CODE"'"}}'
update '{"message":{"message_id":2,"chat":{"id":42,"type":"private"},"text":"/start '"$CODE"'"}}'
wait $PAIR
assert_eq 0 $? "pair exits 0"
assert_eq 42 "$(state "['chat_id']")" "the private chat is paired, not the group"

echo "ctx **bold**" | "$BIN" add --key ACME-1 --question "First?" --option Yes --option No --recommend 1 >/dev/null
"$BIN" telegram run 2>"$SANDBOX/run.err" &
RUN=$!
PIDS+=($RUN)
until_calls "any(x['method']=='sendMessage' and 'First?' in x['body']['text'] for x in c)" 15
assert_eq 0 $? "run sends the card"
MID=$(calls | python3 -c "import json,sys; print([x['message_id'] for x in json.load(sys.stdin) if x['method']=='sendMessage' and 'First?' in x['body']['text']][0])")

"$BIN" telegram run 2>/dev/null
assert_eq 1 $? "a second run is refused"
"$BIN" telegram pair >/dev/null 2>&1
assert_eq 1 $? "pair is refused while the service runs"

update '{"callback_query":{"id":"q1","data":"a:D-1:1","message":{"message_id":'"$MID"',"chat":{"id":42,"type":"private"}}}}'
until_calls "any(x['method']=='editMessageText' and '⏳ 1 — Yes' in x['body']['text'] for x in c)" 10
assert_eq 0 $? "the tap shows the pending answer with undo"
until_calls "any(x['method']=='editMessageText' and '✓ 1 — Yes' in x['body']['text'] for x in c)" 25
assert_eq 0 $? "the card shows the answer after the undo window"
assert_eq "" "$("$BIN" count)" "nothing open"
assert_contains "$(cat "$SANDBOX/events" 2>/dev/null)" "D-1 answered: 1 — Yes" "the hook got the answer"

"$BIN" telegram off >/dev/null
assert_eq False "$(state "['enabled']")" "off from the terminal"
assert_contains "$("$BIN" telegram status)" "service: running" "status sees the service"

kill -TERM $RUN
wait $RUN
assert_eq 0 $? "run exits 0 on SIGTERM"
assert_contains "$(cat "$SANDBOX/run.err")" "telegram: running" "run logs that it started"
if grep -q "T1" "$SANDBOX/run.err"; then fail "the token is not logged" "found in run.err"; else pass "the token is not logged"; fi

echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
