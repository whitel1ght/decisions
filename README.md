# decisions

Your coding agents stop to ask you things: approve this, pick one of these, which file did you
mean. With several sessions running, those questions hide in different terminals.
`decisions` collects them into one queue and shows it as a deck of cards in your terminal; you
answer in a couple of keys, and each answer goes back to the session that asked.

It works with any agent harness that speaks MCP (Claude Code, Codex, Gemini CLI, opencode,
pi, Cursor, …), and the queue is plain Markdown files you own.

```
╭ decisions · demo · 8 waiting · oldest 4h 10m ────────────────────────────────────────────────╮
│ ╭ D-1 · demo · acme-1004 · reply ↩ reply ──────────────────────────────────────── ⏱ 4h 10m ╮ │
│ │                                                                                          │ │
│ │ Reply to the client's question about the export columns?                                 │ │
│ │                                                                                          │ │
│ │ ★ 1  Send the drafted reply                                                              │ │
│ │   2  Change it first (add a note with n)                                                 │ │
│ │   3  I'll answer myself                                                                  │ │
│ │ ✎ 4  Something else… tell the agent                                                      │ │
│ │                                                                                          │ │
│ │ space: context · 5 lines · discussion · 0                                                │ │
│ ╰──────────────────────────────────────────────────────────────────────────────────────────╯ │
│  ╰───────────────────────────────────────────────────────────────────────── D-2 · 2h 05m ─╯  │
│   ╰────────────────────────────────────────────────────────────────────────── D-3 · 45m ─╯   │
│    ╰──────────────────────────────────────────────────────────────────────── D-4 · 20m ─╯    │
│     ╰──────────────────────────────────────────────────────────── D-5 · 14m · +3 more ─╯     │
╰ J/K cards · j/k options · 1-9 answer · ⏎ pick · space flip · ? help ─────────────────────────╯
```

## Install

Rust 1.89 or newer:

```bash
cargo install --git https://github.com/whitel1ght/decisions --tag v0.2.0
decisions --demo        # a made-up queue: try the keys, nothing is read or written
decisions               # the real queue
```

macOS and Linux.

## Connect your agent

```bash
decisions setup claude      # or: codex, gemini, opencode, pi, cursor, generic
```

prints how to register `decisions mcp` in that harness and how long it can wait for you. For
Claude Code it is one line:

```bash
claude mcp add --scope user decisions -e DECISIONS_WAIT=0 -- decisions mcp
```

The server tells the agent to send its questions to you through `ask_user` instead of the chat
(clients that show MCP server instructions pick this up on their own; `setup` also prints an
optional paragraph for `AGENTS.md` / `CLAUDE.md` for those that do not). Questions from every
session land in the same deck, and each card says who asked: `D-7 · claude-code · my-repo`.

| Harness | Waits for a tool call | Raise it with | Honours progress pings |
|---|---|---|---|
| Claude Code | about 28 h; backgrounds the call after 2 min | `MCP_TOOL_TIMEOUT`, per-server `timeout` | resets its idle timer |
| Codex CLI | 60 s | `tool_timeout_sec` | no |
| Gemini CLI | 10 min | per-server `timeout` (ms) | no |
| opencode | 12 h | `timeout.execution` | unverified |
| Cursor | about 60 s, fixed | — | IDE yes, CLI no |
| pi | 60 s | per-server `timeout` (s) | yes |

## How waiting works

`ask_user` files the question and waits up to `DECISIONS_WAIT` seconds (default 50, under the
common 60 s client limit; `0` = no limit), pinging the client every 15 s when it accepts
progress notifications. What you answer in time comes straight back. Otherwise the agent gets
`pending: D-7 is still open — call wait_for_answer("D-7")`, keeps working, and asks again when it
needs the answer. The question lives on disk, not in the request: a client timeout, restart or
reconnect never loses it.

What the agent can get back:

| Result | Meaning |
|---|---|
| `answered: 2 — Push only` (+ `note: …`) | you picked an option |
| `answered in your own words: …` | you answered outside the options |
| `the user asked back: … — answer with reply("D-7", text) …` | you asked a question back |
| `pending: D-7 is still open …` | not answered yet |
| `withdrawn: D-7 (…)` | the question was withdrawn |

## Answers to your questions

You ask a session something while it is busy (mid-task, agents running) and the answer arrives
when you have moved on. The session files it with `tell_user`: an answer card with your question
and its answer, no options, after the questions in the deck (`✉ answer` on the card, `1 answer`
in the header, `✉ N` on the badge). `⏎` marks it seen and files it away (5 s to undo); in
Telegram it comes with one `✓ Got it` button. `tell_user` returns at once: nothing waits on it.

The card records the session that wrote it (`session:`), so you can follow up while it is open:
`a` in the deck (the box opens inside the card), a text reply to the card in Telegram, or
`decisions ask D-7 "and the tests?"`. The follow-up goes back to that session, which answers
with `reply` on the same card; the thread shows under the answer. Neither closes it; marking it
seen does.

## Answering

| Key | |
|---|---|
| `J / K` | next / previous card |
| `j / k` | move between the options (back: scroll) |
| `g / G` | first / last option (back: top / bottom) |
| `1-9` | answer with that option |
| `⏎` | option under the cursor, else ★; on an answer card, got it |
| `space` | flip the card: context and discussion |
| `^d / ^u` | page the back |
| `u` | undo the last answer (5 s) |
| `n` | note on the card, or on the last answer |
| `last row` | something else: tell the agent in your own words |
| `a` | ask the agent a question back; on an answer card (`✉`), follow up on the answer |
| `s` | skip to the end of the stack |
| `o` | open the link (MR, PR, ticket) |
| `r` | reload |
| `?` | all the keys |
| `q / esc` | send pending answers and close |

Each card's border turns blue → yellow → orange → red as it waits.

## Telegram

`decisions telegram` sends the open cards to your Telegram chat while you're away from the
computer, and takes the answers from there. Nothing listens on the network: the service
long-polls Telegram.

1. Create a bot: talk to [@BotFather](https://t.me/BotFather), send `/newbot`, keep the token.
2. Give decisions the token: `export DECISIONS_TELEGRAM_TOKEN=…`, or a command that prints it
   (a password manager, a secrets helper) as `token_cmd` under `[telegram]` in `config.toml`.
3. `decisions telegram pair`, then send the `/start <code>` it prints to your bot.
4. Run the service: `decisions telegram run`, or install it to start at login:
   `decisions telegram setup` prints a launchd agent (macOS) or systemd user unit (Linux) with
   where to save it. The unit never carries the token itself, so the installed service needs
   `token_cmd`; `setup` warns when it is missing or the chat is not paired yet.

While you're away (no keyboard or mouse input for `idle_minutes`), each open card arrives as a
message with its context and a button per option. Tap an option to answer (10 s to undo), or
reply to the card:

| Reply | Means |
|---|---|
| `close it as a duplicate` | an answer in your own words |
| `2 mention the staging run` | option 2, with a note |
| `? which QA run` | a question back to the agent; its reply arrives under the card |

Cards answered in the deck change in the chat too. `/off` and `/on` in the chat, or
`decisions telegram off|on`, stop and restart sending; `/status` and `decisions telegram status`
show where things stand.

Telegram chats are not end-to-end encrypted. With `context = "none"`, messages carry only the
question, the options and the link.

## Configuration

`~/.config/decisions/config.toml` (or `$XDG_CONFIG_HOME/decisions/config.toml`), all optional:

```toml
mode = "always"          # "always": agents send every question here; "on-request": only when you ask them to
wait = 50                # seconds an MCP call waits before returning pending; 0 = no limit
on_answer = ""           # command run on every answer, own-words answer and ask-back
mr_strip_prefix = ""     # removed from repository names in short MR labels
title = "decisions"      # the deck's header name; a second deck can be called something else
badge = "✋"             # the glyph `decisions count --badge` prints before the number

[telegram]
token_cmd = ""           # a command that prints the bot token
idle_minutes = 5         # away after this long without input; 0 = always send
context = "full"         # "full" | "none": include each card's context in its message
```

| Environment | Overrides |
|---|---|
| `DECISIONS_DIR` | where the queue lives (default `~/.local/state/decisions`) |
| `DECISIONS_WAIT`, `DECISIONS_MODE`, `DECISIONS_ON_ANSWER`, `DECISIONS_MR_STRIP`, `DECISIONS_TITLE`, `DECISIONS_BADGE` | the config keys |
| `DECISIONS_SESSION` | a label added to the cards' "who asked" |
| `DECISIONS_TELEGRAM_TOKEN` | the bot token itself (wins over `token_cmd`) |
| `DECISIONS_TELEGRAM_TOKEN_CMD`, `DECISIONS_TELEGRAM_IDLE`, `DECISIONS_TELEGRAM_CONTEXT` | the `[telegram]` keys |

`on_answer` runs with `/bin/sh -c` and gets one JSON line on stdin (10 s limit; a failure warns
and never undoes the answer):

```json
{"event":"answered","id":"D-7","key":"ABC-1","source":"claude-code · repo","option":2,"note":null,"text":"D-7 answered: 2 — Push only.","at":"2026-10-20T11:20:00+03:00"}
```

`event` is `answered`, `told` (own words) or `asked` (you asked back). For example, a desktop
notification: `on_answer = "jq -r .text | xargs -0 notify-send decisions"`.

## The CLI

For scripts and agents without MCP:

| Command | |
|---|---|
| `decisions add --question Q --option A --option B [--recommend N] [--key K] [--mr URL] [--blocks T] [--source S]` | file a question (context on stdin); prints `D-<n>` |
| `decisions list [--json]` | open decisions |
| `decisions show ID` | the decision file |
| `decisions count [--badge]` | how many questions are open (`--badge`: `✋ N`, plus `✉ M` for answer cards) |
| `decisions note --question Q [--key K] [--mr URL] [--source S]` | file an answer card (the answer on stdin); prints `D-<n>` |
| `decisions seen ID` | mark an answer card read and file it under `done/<day>/` |
| `decisions answer ID N [NOTE]` / `tell ID TEXT` / `ask ID TEXT` | answer, answer in your own words, ask back (a follow-up on an answer card) |
| `decisions reply ID TEXT` / `withdraw ID [REASON]` / `close ID` | the asker's side |
| `decisions wait ID [--timeout S]` | block until something happens (exit 0 answered or seen, 3 pending, 4 withdrawn, 5 asked back) |
| `decisions setup HARNESS` | how to connect a harness |

The queue is one Markdown file per decision in `DECISIONS_DIR`; closed ones move to
`done/<day>/`. Writes take a lock and rename a temp file into place, so the deck, the CLI and
any number of MCP servers can share it.

## Development

`cargo test`, `tests/cli.sh` (the CLI on a sandbox store) and `tests/e2e.sh` (the real binary in
a pseudo-terminal, including an MCP question answered in the deck). Design notes:
[docs/spec.md](docs/spec.md).

## Licence

MIT
