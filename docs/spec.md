# decisions — design

Status: approved in conversation 2026-10-20, awaiting spec review.

## What and why

Coding agents (Claude Code, Codex, Gemini CLI, opencode, pi, Cursor, …) stop and ask the user
questions in their chat. With several sessions running, those questions are scattered across
terminals and easy to miss. `decisions` gathers them into one queue, answered in a terminal app
that shows the queue as a deck of cards, and hands each answer back to the session that asked.

Anyone can clone the repo, build one binary, register it with any harness that speaks MCP, and
have every session's questions land in the same deck.

**Success:**

- A new user goes from `git clone` to an agent's question showing in the deck with one build
  command and one registration command, without editing their agent instructions.
- Answering takes two or three keys; the longest-waiting question is obvious at a glance.
- An answer is never lost or delivered twice, whatever the client does (time out, restart,
  reconnect, cancel).

**Out of scope (for now):** syncing across machines, a queue shared between people, desktop
notifications, Windows, crates.io and Homebrew releases.

## One binary

`decisions` (Rust, MIT, macOS and Linux, Rust 1.89+):

| Command | What it does |
|---|---|
| `decisions` | the deck (terminal app) |
| `decisions --demo` | the deck on a made-up in-memory queue; touches nothing |
| `decisions mcp` | MCP server over stdio |
| `decisions add --key K [--mr URL] [--blocks T] [--source S] --question Q --option O… [--recommend N]` | file a question; context on stdin; prints `D-<n>` |
| `decisions list [--json]` | open decisions, blocking first, then oldest first |
| `decisions show ID` | the decision file |
| `decisions count [--badge]` | number open (nothing when none; `--badge` prints `✋ N`) |
| `decisions answer ID N [NOTE]` | pick option N |
| `decisions tell ID TEXT` | answer in your own words, none of the options |
| `decisions ask ID TEXT` | ask the asker a question back |
| `decisions reply ID TEXT` | the asker answers that question |
| `decisions withdraw ID [REASON]` | no longer needed |
| `decisions note --question Q [--key K] [--mr URL] [--source S]` | file an answer card (the answer on stdin); prints `D-<n>` |
| `decisions seen ID` | the user has read an answer card: file it under `done/<day>/` |
| `decisions close ID` | file an answered or withdrawn decision under `done/<day>/` |
| `decisions wait ID [--timeout S]` | block until something happens; prints the outcome (as the MCP tools word it); exit 0 answered, 3 still pending, 4 withdrawn, 5 the user asked back |
| `decisions setup <harness>` | print how to register the MCP server in that harness |
| `decisions telegram run\|pair\|on\|off\|status\|setup` | the Telegram bot (`docs/telegram.md`) |

Exit codes for the other commands: 0 ok, 1 a refused operation (unknown id, already answered,
option out of range), 2 usage error.

## The store

The only shared state. One Markdown file per decision in `$DECISIONS_DIR` (default
`$XDG_STATE_HOME/decisions`, i.e. `~/.local/state/decisions`): `D-<n>.md`, and closed ones under
`done/<YYYY-MM-DD>/`.

```markdown
---
id: D-7
day: 2026-10-20
key: ACME-1001
mr: https://gitlab.com/group/repo/-/merge_requests/870
asked: 2026-10-20T11:04:08+03:00
blocks: reply
status: open            # open | answered | withdrawn | seen
source: claude-code · repo-name · session-label
kind: answer            # only on answer cards
session: 3f9c…          # only on answer cards: the writer's session id
---
## Question
Push the fix and post the reply?
## Context
<markdown>
## Options
1. Push and post (recommended)
2. Push only
## Discussion
- 11:04 you: which QA run?
- 11:05 session: the staging one
## Answer
2 — Push only
note: mention the staging run
at: 2026-10-20T11:20:00+03:00
```

- An own-words answer is `## Answer` `your words — <text>`.
- An answer card (`kind: answer`, filed by `tell_user` or `decisions note`) reports a session's
  answer to the user's question: `## Question` is the user's question, `## Context` the answer,
  no `## Options`. It cannot be answered or told; `seen` sets `status: seen`,
  `## Answer` `seen` with `at:`, and moves it to `done/<day>/` at once. The same source filing
  the same question while its card is open replaces the answer. Answer cards list after every
  question; `count` leaves them out (`count --badge` adds `✉ M`). `kind` is written only when
  non-empty, after `source`.
- Follow-ups: while an answer card is open, `ask` appends the user's follow-up (`you:`) to
  `## Discussion` and the session answers with `reply` (`session:`); neither closes the card.
  `session` (written by `tell_user`, only when non-empty) names the session that wrote the card,
  so a follow-up can reach it: `DECISIONS_OWNER` if set, else `CLAUDE_CODE_SESSION_ID`, else
  `pid:<parent pid>` of the MCP server. The `asked` event carries it as `session`.
- `key`, `mr`, `blocks`, `source` may be empty. `source` is written last and only when
  non-empty, so a file without it stays byte-identical to the older Python format. Options: 2 to 9, at most one `(recommended)`.
- Numbers never repeat: the next id is one more than the highest in the folder and `done/`.
- Every write takes an exclusive lock on `$DECISIONS_DIR/.lock`, writes a dot-prefixed temp
  file and renames it into place, so readers never see half a file and concurrent writers never
  lose each other's changes.
- `list --json` returns every field: `id, key, mr, mr_short, source, kind, question, asked, age,
  blocks, question_back, options[{n,text,recommended}], recommended, context,
  discussion[{at,who,text}], session, discussion_count, followup`. `question_back` is true when
  the last discussion entry is the user's; `followup` is that entry's text on an answer card,
  else `""`; `session` is `""` when the card has none.

## Config

`$XDG_CONFIG_HOME/decisions/config.toml` (default `~/.config/decisions/config.toml`), all optional:

```toml
mode = "always"        # "always" | "on-request": what the MCP server tells agents
wait = 50              # seconds an MCP call waits before returning pending; 0 = no limit
on_answer = ""         # command run on every answer, own-words answer and ask-back
mr_strip_prefix = ""   # removed from repository names in short MR labels, e.g. "acme-"
```

Environment overrides: `DECISIONS_DIR`, `DECISIONS_WAIT`, `DECISIONS_MODE`,
`DECISIONS_SESSION` (a label added to `source`), `DECISIONS_ON_ANSWER`, `DECISIONS_MR_STRIP`.

## The `on_answer` hook

Run after `answer`, `tell` and `ask` (from the CLI, the deck or the MCP server), with one JSON
object on stdin:

```json
{"event": "answered", "id": "D-7", "key": "ACME-1001", "source": "claude-code · repo",
 "option": 2, "note": "mention the staging run", "text": "D-7 answered: 2 — Push only. Note: mention the staging run.",
 "at": "2026-10-20T11:20:00+03:00"}
```

- `event` is `answered`, `told` (own words: `option` null, `text` "D-7 answered in your own
  words: …") or `asked` (`text` "D-7: the user asks: …").
- It runs with a 10 s limit. A failure or timeout never undoes the change: the CLI prints a
  warning to stderr; the deck shows a toast.
- It is how a user plugs the queue into their own tooling (an inbox, a notifier, a chat).

## The MCP server

`decisions mcp`, JSON-RPC over stdio.

**Instructions** (the `instructions` field of `initialize`, which clients that support it add to
the agent's system prompt), by `mode`:

- `always`: "When you need a decision, approval or clarification from the user, call `ask_user`
  instead of asking in the chat. Keep working on anything that does not depend on the answer;
  when you need it, call `wait_for_answer`. When you answer a question the user asked while
  you were busy with other work, also file your answer with `tell_user`, so they find it in the
  same queue instead of scrolling back for it."
- `on-request`: "If the user asks you to send your questions to the decisions app, use
  `ask_user` for them from then on instead of asking in the chat."

**Tools:**

| Tool | Arguments | Result |
|---|---|---|
| `ask_user` | `question`; `options` (2–8 strings); optional `recommended` (1-based), `context` (markdown), `link` (URL), `ticket`, `blocks` | an outcome (below) |
| `wait_for_answer` | `id`; optional `timeout_seconds` | an outcome |
| `reply` | `id`, `text` | ok; answers a question the user asked back, or a follow-up on an answer card |
| `withdraw` | `id`; optional `reason` | ok |
| `tell_user` | `question` (the user's), `answer` (markdown); optional `link`, `ticket` | `filed` with the id, at once |

A `ticket` that is not a `KEY-123` key goes into the context as `Ticket: …`. Asking the same
open question again (same ticket and question) returns the existing id instead of a second card.

`ask_user` adds an "own words" slot automatically: the user can always answer outside the
options, so the agent never adds an "Other" option itself.

**Outcomes** (text for the model, with the same fields as structured content):

- `answered: 2 — Push only` (and `note: …`)
- `answered in your own words: close it as a duplicate`
- `the user asked back: which QA run? — answer with reply("D-7", text), then call wait_for_answer("D-7")`
- `pending: D-7 is still open — call wait_for_answer("D-7") when you need the answer`
- `withdrawn: D-7 (reason)`
- `seen: the user has read D-7` (an answer card)

**Waiting:**

- A call waits up to `wait` seconds (or `timeout_seconds`, never more than `wait` unless `wait`
  is 0), polling the store (file watch, plus a 1 s re-check).
- When the request carries a progress token, the server sends `notifications/progress` every
  15 s with a rising value.
- On timeout it returns `pending` as a normal result, never an error.
- An ask-back seen while waiting returns at once. It returns only once per entry: the server
  remembers, per id, how many discussion entries it has reported.
- Nothing lives in the request: a client timeout, cancel, restart or reconnect leaves the
  question in the store, and `wait_for_answer` picks it up later from any server process.

**Source:** `<clientInfo.name> · <basename of the server's working directory>` plus
` · $DECISIONS_SESSION` when set.

## The deck

The existing app (deck of cards, J/K cards, j/k options, 1–9 answer with a 5 s undo, the own-words
slot, notes, flip to context and discussion, ask back, skip, open link, help, demo, live age
colours and tachyonfx animations), with these changes:

- It reads and writes the store directly, in process, with a watcher on `$DECISIONS_DIR`; no
  subprocess.
- Card headers show the source: `D-7 · claude-code · repo · acme-1001 · reply`.
- `on_answer` failures show as a toast.
- An answer card shows `you asked: <question>`, the answer, and `⏎  Got it`; ⏎ throws it with
  the same undo window and marks it seen. Numbers, `a` and `n` only say so.

## `decisions setup <harness>`

Prints, for `claude`, `codex`, `gemini`, `opencode`, `pi`, `cursor`, and `generic`:

1. the command or config block that registers `decisions mcp`, with the timeout setting that
   harness needs (e.g. `claude mcp add decisions -- decisions mcp`; a `[mcp_servers.decisions]`
   block with `tool_timeout_sec` for Codex);
2. how waiting behaves there (from the table below);
3. an optional paragraph for `AGENTS.md` / `CLAUDE.md`, marked optional, for harnesses that do
   not show server instructions.

Known client behaviour (checked 2026-10-20; re-check while building):

| Harness | Default tool timeout | Raise with | Honours progress pings |
|---|---|---|---|
| Claude Code | about 28 h; 30 min idle on stdio; backgrounds the call after 2 min | `MCP_TOOL_TIMEOUT`, per-server `timeout` | resets the idle timer |
| Codex CLI | 60 s | `tool_timeout_sec` | no |
| Gemini CLI | 10 min | per-server `timeout` (ms) | no |
| opencode | 12 h | `timeout.execution` | unverified |
| Cursor | about 60 s, fixed | — | IDE yes (since 2026-07), CLI no |
| pi | 60 s | per-server `timeout` (s) | yes |

## Testing

- **Store:** numbering, ordering, the file format round trip, concurrent writers (15 runs of a
  reply racing an answer, nothing lost), atomic writes.
- **CLI:** the real binary in a temporary store: every command, every exit code, `wait`, `setup`.
- **MCP:** an in-process test client over pipes:
  - `initialize` returns the instructions for each `mode`; `tools/list` matches the expected
    schemas.
  - `ask_user`: answered within the wait, `pending` after it, an ask-back, an own-words answer,
    a withdrawal.
  - Progress is sent only with a token.
  - A question survives the server being killed mid-wait, and a new server's
    `wait_for_answer` returns its answer.
- **Hook:** the exact JSON per event; a failing or slow hook warns and the change stands.
- **The deck:** state, drawing and effect tests; the source on the card header.
- **End to end**, the binary in a pseudo-terminal:
  - answer, undo, note, own words, ask back, quit with an answer pending, signals, `--demo`;
  - an MCP `ask_user` answered in the deck returns that answer.
- **CI:** GitHub Actions on macOS and Linux runs `cargo test`, clippy (`-D warnings`), rustfmt
  `--check` and the end-to-end script.

## Release

`0.1.0`, tagged. Install: `cargo install --git https://github.com/whitel1ght/decisions --tag
v0.1.0`. Nothing is pushed, published or tagged on GitHub before the owner tries a local build
and says so.

## README

1. What it is, and a screenshot from `decisions --demo`.
2. Install, then `decisions --demo`.
3. Connect a harness: `decisions setup <harness>`, and the client table.
4. How waiting works, and what the agent sees.
5. Keys.
6. Config, the environment overrides, and the `on_answer` JSON.
7. The CLI, for scripts.
8. Licence (MIT).
