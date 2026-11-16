# Activity view: who is working, who is idle

Status: for approval (2026-11-16). Decisions behind it: D-943 (why), D-944 (where), D-945
(sessions waiting on the user get their own state). Reviewed once by a builder check.

## Intent

The user runs several agent sessions at once: Claude Code (interactive and background) and pi.
They want to be sure none of them sits idle without having to go and check each one. When one is
idle, they may want to hand it a new task right away (D-943).

So the deck gets:

1. **An activity screen**, opened with one key and built like the stats screen (`S`). It shows
   every running session, whether it is working or idle and for how long, its running subagents,
   and what each is doing right now.
2. **A loader on the deck's main view**: a spinner in the deck's header that turns whenever at
   least one session or agent is working (D-944: "on main view loader needed to see if at least
   something is running"). The screen itself is not shown all the time; the spinner is.

## Goals

- At a glance, from the deck's main view: is anything running? (spinner, plus a count)
- One key away: every session, working or idle, with idle sessions easy to spot.
- From an idle session's row, one key to type it a new task (the existing prompt line).
- Counts subagents: a session waiting on its subagents is not idle.
- A session waiting on the user (a question card, a permission prompt, `needs input`) is neither
  working nor idle: it shows as **waiting on you** (✋) and never keeps the spinner turning
  (D-945).

## Non-goals

- Controlling agents: no stop, kill or restart from the screen.
- History or charts of activity over time (that is stats' job).
- Sessions on other machines.
- Exact real-time state. A few seconds' lag is fine.

## Anti-goals (a failure even if it works)

- The deck gets slower or less responsive: no scan may block the UI thread.
- A session shows as idle while it is actually working (or the reverse) often enough that the
  user goes back to checking windows by hand. When the state can't be known, say "unknown", never
  guess "idle".
- The screen becomes a second dashboard to watch: it opens on a key and closes on esc, like stats.

## Constraints

- Rust, in `~/projects/decisions`. The tests run in CI on macOS and Linux (`cargo test`, clippy
  `-D warnings`, rustfmt, `tests/e2e.sh`). The app is public (`whitel1ght/decisions`), so
  machine-specific paths must stay defaults that can be overridden, and the feature must degrade
  quietly where Claude Code or pi is absent.
- The deck already reads sessions in `src/sessions.rs` (`running`: the registry merged with
  `claude agents --json`, 3 s limit, about 90 ms). Reuse it; don't add a second session model.
- Transcripts can be hundreds of MB. Read them from the end only (as `src/colors.rs` does after
  its first scan).

## What can be known (checked 2026-11-16)

- **Claude Code sessions:** `claude agents --json` lists interactive and background sessions with
  `pid`, `sessionId`, `name`, `cwd`, `status` (`busy`/`idle`) and, for background ones, `state`
  (`working`/`done`/…). `sessions::state_label` already maps these.
- **Claude Code subagents:** `~/.claude/projects/<slug>/<sessionId>/subagents/agent-<id>.jsonl`
  plus `agent-<id>.meta.json` with `description`, `agentType` and `finished: true` once done.
  The last `tool_use` in the jsonl shows what it is doing.
- **What a Claude session is doing:** the last `assistant` record in
  `~/.claude/projects/<slug>/<sessionId>.jsonl`: its last `tool_use` (name plus a short form of
  the input: a Bash `description`, an Edit `file_path`, …), else the first line of its text.
- **pi sessions:** registered in `<dir>/sessions/<pid>.json` by `decisions mcp`, with no state.
- **pi-sub agents:** `~/.local/state/pi-sub/<id>/` with `meta.json` (`label`, `role`, `model`),
  `events.jsonl`, and `exit.json` once finished.

## Approaches

- **A. In the deck (chosen, D-944).** A screen like stats, the spinner in the deck's header, and
  ⏎ on a session opens the prompt line aimed at it. It reuses the deck's session list and prompt
  line, which is exactly the "give an idle session a task" step.
- **B. A separate fzf popup script in dotfiles.** Cheaper, but giving a session a task means
  jumping to its window, and the loader would live somewhere else.
- **C. Extend session-picker (`` ` j``).** Same limit as B, and it sees only tmux windows.

## Design

### Activity model (`src/activity.rs`, new)

`scan(dir, now) -> Activity` returns a `Vec<SessionActivity>`, sorted with idle sessions first,
then waiting on you, then working, then unknown, each group by name:

- `session`: the `sessions::Session` (name, owner, pid, harness, cwd).
- `state`: `Idle | WaitingOnYou | Working | Unknown`, and `since`: when that state began, as far
  as it can be told (the transcript's last record time for idle; the card's `asked` time for
  waiting on you; the first record of the current turn for working; none for unknown).

A session is **waiting on you** (D-945) when, checked in this order:
1. it has an open question card in this deck: a question card's `source` matches the session
   (harness · folder name) and the session is the one the MCP server is serving. Question cards
   carry no `session` today, so `ask_user` starts stamping `session:` on question cards too, as
   it already does on answer cards (the owner id); older cards without it are matched by source
   only when exactly one running session has that source;
2. Claude reports `needs_input` (background) or the transcript's last record is a tool call
   waiting for permission (interactive; if that can't be read, this case is skipped).

It wins over working: a session blocked in `wait_for_answer` is reported busy by Claude Code,
but the user is the one it waits on.
- `doing`: one line, as described above, with its timestamp.
- `agents`: the session's running subagents, each with a label (meta `description`, or pi-sub
  `label`), a `doing` line and its age.

A session counts as **working** when its own state is working **or** any of its subagents runs
**or** it waits on its own background job (a background shell command, Monitor, a scheduled
wakeup): it will wake up by itself, so it is not free for a new task. Detecting those jobs is
part of the work (look for an open background task in the transcript tail; if it can't be told,
the session keeps the state Claude reports). *The agent's call, from the why in D-943.*

A subagent with no `finished` flag whose jsonl hasn't changed for `AGENT_STALE` (10 min) is
shown as **unknown**, not dropped: a long silent command writes nothing. If a session's only
running agents are unknown and the session itself is idle, the session is unknown too.
*The agent's call, from the anti-goal "never guess idle".*

`Activity::working()` counts working sessions plus working agents in the "other agents" group;
`Activity::waiting()` counts sessions waiting on you; `Activity::unknown()` counts unknown
sessions and agents. *The agent's call.*

The scan runs on a background thread, the way `LoadStats` does, never on the UI thread. Results
arrive as an `Event`.

### Sources, each optional

- Claude sessions and states: `sessions::running` (no change).
- Claude transcript reading: a new reader that looks up `<sessionId>.jsonl` under
  `$CLAUDE_CONFIG_DIR/projects` (default `~/.claude/projects`) by file name, and reads only the
  tail (grow the window until it holds a complete `assistant` record, up to a cap).
- Claude subagents: `meta.json` files whose `finished` is not true and whose jsonl changed within
  `AGENT_STALE` (10 min; a crashed session never writes `finished`).
- pi sessions: state `Unknown` until pi reports one. **Add-on in dotfiles:** pi's daily extension
  writes `state` (`working` on `agent_start`, `idle` on `agent_end`) and `since` into its own
  registry file, through a new `decisions session-state working|idle` CLI command that rewrites
  the registry file under its lock. `Session` gains optional `state`/`since` fields; old files
  without them read as unknown.
- pi-sub agents: folders under `$PI_SUB_DIR` (default `~/.local/state/pi-sub`) with `meta.json`
  and no `exit.json`. Today's `meta.json` records no parent, so they show in an "other agents"
  group. **Add-on in dotfiles (same step as pi state):** pi-sub writes the spawning pi process's
  pid as `parent_pid` in `meta.json`, and the screen attaches an agent to the session with that
  pid. *The agent's call: both dotfiles changes are in scope, as the last rollout step.*
- `Session` already has `state: String` (saved in the registry, overwritten by `merge` for Claude
  sessions). Reuse it and add `since`. The registry has no lock today; `session-state` takes one
  (same pattern as the queue's `.lock`).

### The screen

`w` opens it from the deck (the free key next to `S`; both are "views"). `decisions --activity`
starts the deck on it. Esc, `w` or `q` returns. It refreshes every 3 s while open.

```
 activity · 2 working · 1 waiting on you · 1 idle · 1 unknown  refreshed 2s ago
 ─────────────────────────────────────────────────────────────────────────────
 ○ main                      idle 14m   ~/projects        last: "Plan saved."
 ✋ the system               waiting 6m ~/projects        D-945: how should a session…
 ⠹ ACME-19302                working 3m ~/projects        Bash: run the plan-pick tests
     └ ⠹ Review session colours diff   40s   Read: src/colors.rs
 ⠹ ACME-18384                working 22m ~/projects/…     Edit: src/app.rs
 ? pi · articles             unknown     ~/projects/articles
 ─────────────────────────────────────────────────────────────────────────────
 j/k move · ⏎ give it a task · esc back
```

- Idle rows come first and are the only ones in the accent colour, since they are what the user
  is looking for.
- ⏎ on a **waiting on you** row closes the screen and puts that session's oldest open card on
  top of the deck, ready to answer (D-945). On a permission prompt there is no card, so ⏎ says
  `answer it in the session` in a toast.
- ⏎ on any other session row opens the prompt line aimed at that session (the same path as `a` on an
  answer card). On a working session the message still goes; it arrives when the turn ends, as
  the prompt line already does. If a draft is already aimed at another session, ⏎ does not move
  it: a toast says `draft is for <name>: send or clear it first`. Esc or sending returns to the
  activity screen. *The agent's call: it keeps the rule that a draft never moves.*
- Colours: the session name wears the session's card colour when it has one (`colors.rs`
  unchanged, so sessions without cards have none); the state word of an idle row is in the
  accent colour, and nothing else is. *The agent's call.*

### The loader on the main view

- The deck's header shows a braille spinner and `N working` while `working() > 0`, and a dim
  `? N` while `unknown() > 0`, so an unknown pi session never reads as "nothing running".
  Sessions waiting on you do not turn the spinner; they add `✋ N` (D-945), which on the main view
  sits next to the cards they wait on. Nothing shows while everything is idle. Frames advance on the deck's existing frame timer.
  *The `? N` part is the agent's call.*
- A background scan runs every 5 s while the deck is open (only the states; the `doing` lines are
  read only while the screen is open).
- `decisions count --badge` gains a working segment for the tmux status bar: a steady glyph and
  `N` while anything works (`icons`: text `⟳`, nerd U+F110 spinner). It is not animated, since
  tmux redraws only every 5 s. **The agent's call; drop it if the deck's spinner is enough.**

### Demo

`--demo` gains made-up activity (one idle session, one waiting on a demo card, two working ones,
one with a subagent, one unknown) so the
screen can be tried safely, like the rest of the demo queue.

## What is left to the builder

- Exact column widths, truncation and the "how long" format.
- How far back to read a transcript tail before giving up (cap it, say 2 MB).
- Whether `since` for a working Claude session comes from the transcript or from the first scan
  that saw it working.
- Test shape: unit tests on `activity.rs` with fixture transcripts and meta files in a temp dir;
  `app.rs` key tests like the stats ones; an e2e check that `w` opens and esc closes the screen.

## Rollout

1. `activity.rs` and the screen, with Claude sessions and subagents.
2. The header spinner and the `count --badge` segment.
3. pi state: `decisions session-state` plus the pi daily extension change in dotfiles.
4. Docs: README, `docs/spec.md` section, wiki `work/system/decisions-app.md` and
   `tools/terminal.md`, plus a tmux key (`` ` W`` → `decisions --activity`) if wanted.
