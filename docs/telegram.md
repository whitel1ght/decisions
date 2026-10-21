# decisions telegram — design

Status: approved in conversation 2026-10-21, awaiting spec review.
Builds on `docs/spec.md` (the store, the hook, the deck).

## What and why

The deck works only at the computer. `decisions telegram` is a small service that sends the open
decisions to the user's Telegram chat while they are away from the computer, and takes answers
back from there: a tap on an option, a reply in their own words, or a question back to the agent.
Answers go through the store and the `on_answer` hook exactly like answers from the deck, so the
asking session cannot tell the difference.

**Success:**

- Away from the computer, a new decision reaches the phone within seconds; one tap or one reply
  answers it, the asking session continues, and the deck shows the card as answered.
- At the computer, Telegram stays quiet.
- The bot can be switched off and on from the terminal and from the chat.
- Nobody but the paired chat can read or answer anything.
- Setting it up takes a bot token from @BotFather, one `pair` command and one service file.

**Out of scope:** group chats, several users, webhooks (it long-polls, so nothing listens on the
network), live-updating the "waiting" age in sent messages, Windows.

## Commands

| Command | What it does |
|---|---|
| `decisions telegram run` | the service: long-polls Telegram and watches the store until stopped |
| `decisions telegram pair` | links the user's private chat to the bot (below) |
| `decisions telegram on` / `off` | switches sending on or off; the service keeps running |
| `decisions telegram status` | on or off, paired or not, whether the service runs, whether the user counts as away, open cards sent, the last error |
| `decisions telegram setup` | prints a launchd agent (macOS) or a systemd user unit (Linux) that runs `decisions telegram run` |

Exit codes: 0 ok (`run` also exits 0 when stopped by SIGTERM or SIGINT), 1 a refused operation (not paired, no token, another `run` or `pair` holds the
lock, pairing timed out), 2 usage error.

## Config

A new optional section in `config.toml`:

```toml
[telegram]
token_cmd = ""          # a command that prints the bot token (run with /bin/sh -c)
idle_minutes = 5        # the user is away after this long without keyboard or mouse input; 0 = always away
context = "full"        # "full" | "none": include each card's context in its message
```

Environment overrides: `DECISIONS_TELEGRAM_TOKEN` (the token itself, wins over `token_cmd`),
`DECISIONS_TELEGRAM_TOKEN_CMD`, `DECISIONS_TELEGRAM_IDLE`, `DECISIONS_TELEGRAM_CONTEXT`, and
`DECISIONS_TELEGRAM_API` (the Bot API base URL, default `https://api.telegram.org`; for tests).

- The token is never printed, logged, or written to any file. Every error text that leaves the
  API client has the token replaced by `<token>` (HTTP client errors include the request URL,
  which contains it).
- `token_cmd` is run again whenever the token is needed after a 401, so a rotated secret is picked
  up without a restart.
- `context = "none"` exists because Telegram chats are not end-to-end encrypted: with it, messages
  carry only the header, the question, the options and the link.

## State

`$DECISIONS_DIR/telegram.json`, mode 0600, written as a temp file renamed into place, under an
exclusive lock on `$DECISIONS_DIR/.telegram.lock`:

```json
{
  "chat_id": 123456789,
  "enabled": true,
  "offset": 4711,
  "messages": {"D-7": {"message_id": 42, "done": false, "discussion_seen": 2, "text": "…", "html": true, "replies": [57]}},
  "pending": {"D-7": {"option": 2, "note": "", "due_ms": 1792570810000, "message_id": 42}},
  "last_error": null
}
```

- `offset` is the next Telegram update id, so a restart neither replays nor loses updates.
- `messages` records which card went to which message, the text it was sent with (edits append a
  status line to it; an answered card is no longer in the open list to render again), and the
  ids of the bot's thread replies under it (so a reply to those counts as a reply to the card).
  A card is sent at most once.
- `pending` holds tapped answers inside their undo window; on start, overdue ones are saved
  first.
- `run` and `pair` additionally hold `$DECISIONS_DIR/.telegram.run.lock` for their lifetime, so
  only one of them talks to Telegram at a time (two `getUpdates` callers make Telegram return
  409). `on`, `off` and `status` only take the state lock.
- The store ignores these files (it reads `D-<n>.md` only).

## Pairing

`decisions telegram pair`:

1. Gets the token, calls `getMe`, and prints the bot's name. A bad token is refused (exit 1).
2. Takes the run lock; when `run` holds it, says to stop the service first (exit 1).
3. Prints `Send /start 482913 to @<bot> within 5 minutes.` The code is 6 random digits, used once.
4. Long-polls until a message from a **private** chat whose text is exactly `/start <code>`;
   saves its `chat_id`, sets `enabled: true`, replies "Paired. You'll get decisions here while
   you're away." and exits 0. Any other message is skipped. After 5 minutes: exit 1.
5. Pairing again replaces the chat and clears `messages` and `pending`.

From then on every update from any other chat, and every non-private chat, is dropped without a
reply.

## The service

`decisions telegram run` refuses to start (exit 1) without a token source or a paired chat. Then:

- **One thread, one loop.** Each turn: save overdue pending answers, bring sent cards up to date,
  send new cards, then long-poll `getUpdates` with a 5 s timeout and handle what arrives. An
  update ends the poll at once; otherwise the loop turns every 5 s, so a new card goes out within
  about 5 s. The store is re-read each turn (a few small files), so no file watcher is needed.
- **State on disk is the truth.** Every change to `telegram.json` is a read-modify-write under the
  state lock, so `decisions telegram on`/`off` from the terminal is never overwritten.
- **Away:** idle time is read each turn.
  - macOS: `HIDIdleTime` from `ioreg -c IOHIDSystem`.
  - Linux: `xprintidle` when it exists.
  - When neither works, the user counts as always away and one warning is logged.
  - Away means idle ≥ `idle_minutes`; `0` means always away.
- **Sending:** when enabled and away, every open card without a `messages` entry is sent, in the
  store's order (blocking first, then oldest). Coming back to the computer sends nothing more;
  messages already sent stay answerable.
- **Off:** `enabled: false` stops new sends only. Updates are still handled, so `/on`, taps and
  replies on sent cards keep working, and sent messages are still edited.
- **Edits:** each turn, every sent card that is not `done` is checked against the store:
  - answered → "✓ 2 — Push only", buttons removed, `done`;
  - answered in own words → "✓ your words — …", `done`;
  - withdrawn → "withdrawn: reason", `done`;
  - the file is gone (closed or deleted) → buttons removed, `done`;
  - new discussion entries from the session (beyond `discussion_seen`) → each is sent as a new
    message replying to the card's message: "session: the staging one".
- **Pending answers:** each turn, those past `due_ms` are saved with `store.answer`, the hook is
  run, and the message is edited as above.
- **Stopping:** SIGTERM or SIGINT ends the loop after the current step; pending answers stay in
  the state and are saved on the next start.

## Messages

HTML parse mode.

```
✋ D-7 · ACME-1001 · reply · waiting 3m
claude-code · acme-backend

<b>Push the fix and post the reply?</b>

<context>

🔗 backend!870
Reply: text = own words · "2 text" = option 2 with a note · "? text" = ask back
```

- The header is the deck's card header (id, key, blocks, age at send time); the second line is the
  `source`, left out when empty. The link line uses the deck's short MR label; it is left out when
  there is no link.
- The context is converted from Markdown to Telegram's HTML subset:
  - `**bold**` → `<b>`, `` `code` `` → `<code>`, fenced blocks → `<pre>`, `[text](url)` → `<a>`;
  - headings become bold lines; everything else is plain text;
  - `&`, `<`, `>` are escaped everywhere.
- The context is cut at 3000 characters of source with "…"; the whole message never exceeds 4096
  characters.
- When Telegram rejects the HTML (400 "can't parse entities"), the same message is sent as plain
  text.
- **Buttons:** one row per option, `1 · Push and post ★` (★ on the recommended one). Callback data
  is `a:D-7:2`, and `u:D-7` for undo.

## Answering

- **Tap an option:** the bot answers the callback, records a `pending` answer due in 10 s, and edits
  the message to "⏳ 2 — Push only" with one `↩ Undo` button.
  - Undo removes the pending answer and restores the message and its buttons.
  - A tap on another option while one is pending replaces it and restarts the 10 s.
  - A tap on a card that is already done just answers the callback with "already answered".
- **Reply to a card's message with text**, read in this order:
  1. `? <text>`: an ask-back (`store.ask`).
  2. `<n> <text>` or `<n>` alone, `n` an option number of the card: an answer with the text as the
     note. It is saved at once (typing is not a mis-tap) and cancels any pending tap.
  3. Anything else non-empty: an own-words answer (`store.tell`).

  A number outside the card's options is read as own words.
- **Text that replies to nothing:** when exactly one sent card is not `done`, it goes to that card;
  otherwise the bot answers "Reply to the card you mean." Text replying to a done card gets
  "D-7 is already answered."
- **Every answer and ask-back runs the `on_answer` hook** with the same JSON as the CLI. A hook
  failure keeps the answer and is reported in a reply: "⚠ hook failed: …".
- **A race with the deck** (the store refuses because the card is already answered): the message
  shows the stored answer with "(in the deck)", and the pending entry is dropped.

## Chat commands

- `/on`, `/off`: set `enabled`.
- `/status`: on or off, away or not, and the open cards (`D-7 · question`, marked sent or not yet).
  Telegram has no links to messages in private chats, so it lists them instead.
- `/help`: the reply forms and the commands.
- Anything else starting with `/`: the help.

## Failures

- **Network errors, 5xx, 429:** retry with backoff from 1 s doubling to 60 s, using Telegram's
  `retry_after` when given. Failed sends and edits are retried on the next turn. The service never
  exits over these. The error goes to stderr and to `last_error`; the next success clears it.
- **401 or a token command that fails:** logged once, kept in `last_error`, the token is fetched
  again every 60 s; the service keeps running so fixing the secret is enough.
- **403 (the user blocked the bot):** logged, kept in `last_error`, retried every 60 s.
- **Unknown or malformed callback data or updates:** ignored.
- **A store write refused for any other reason** (unknown id, option out of range): the reply
  says why, in the CLI's words.

## Code

- `src/telegram/api.rs`: the Bot API client on `ureq` (blocking, rustls): `get_me`, `get_updates`,
  `send_message`, `edit_message_text`, `edit_message_reply_markup`, `answer_callback_query`. It is
  the only code that touches the network, and it redacts the token. A trait `Api` over it lets
  the loop run against a fake.
- `src/telegram/render.rs`: pure functions: card → text and buttons, the done and pending forms,
  Markdown → Telegram HTML, cutting.
- `src/telegram/reply.rs`: pure parsing of reply text and callback data.
- `src/telegram/state.rs`: `telegram.json` load and save under the lock.
- `src/telegram/bot.rs`: one turn of the service (sending, syncing, updates), generic over `Api`,
  the clock and the idle source.
- `src/telegram/service.rs`: the loop around turns (backoff, reconnecting with a fresh token,
  `last_error`, stopping) and `pair`.
- `src/telegram/mod.rs`: the commands and `setup`.
- `src/idle.rs`: idle seconds on macOS and Linux.
- `src/config.rs`: the `[telegram]` section and its overrides.

## Testing

- **render:** HTML escaping, each Markdown conversion, cutting at 3000 and the 4096 cap,
  `context = "none"`, the ★, the source and link lines left out when empty, the done and pending
  forms.
- **reply:** `? q`, `2 note`, `2`, `9` on a 2-option card, plain text, empty text, whitespace;
  callback data valid and malformed.
- **state:** round trip, mode 0600, `on` and `off` while the service writes.
- **bot, against a fake `Api`, a fake clock and a fake idle source:**
  - sends only when enabled and away; coming back sends nothing; going away again sends only
    unsent cards, in store order;
  - a tap then 10 s saves the answer and runs the hook; undo inside 10 s saves nothing; a second
    tap replaces the first; a restart with an overdue pending answer saves it;
  - text replies of each form; text replying to nothing with one and with two cards open;
  - answered in the deck, withdrawn, closed: the message is edited and `done`;
  - a session reply to an ask-back is threaded under the card once;
  - updates from another chat or a group are ignored;
  - a restart sends nothing twice and does not replay updates;
  - 429 with `retry_after`, a 5xx, a 401 then a fixed token;
  - an error that includes the token is logged without it.
- **pair:** right code from a private chat; wrong code; right code from a group; timeout; refused
  while `run` holds the lock.
- **End to end** (`tests/telegram.sh`): the real binary with `DECISIONS_TELEGRAM_API` pointing at
  a small fake Bot API server in Python. It pairs, `run` sends an added card, a callback answers
  it after the undo window, and the store, the hook output and the edited message all show the
  answer.
- **CI:** the existing workflow runs all of it on Ubuntu and macOS. A check against the real
  Telegram is a manual step before release.

## README

A "Telegram" section: creating the bot with @BotFather, the token sources, `pair`, `setup` and
installing the service, the reply forms, `on`/`off`, `context = "none"` and why.

## Release

`0.2.0`, tagged after a manual check with a real bot. Nothing is pushed before the owner says so.
