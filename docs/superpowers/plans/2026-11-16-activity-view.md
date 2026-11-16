# Activity View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The deck shows which agent sessions are working, waiting on the user, idle or unknown: a
spinner in its header all the time, and a full screen on `w`.

**Architecture:** Pure readers (`transcript.rs`, `agents.rs`) turn Claude transcripts and pi-sub
folders into `AgentActivity`/`Doing`. `activity.rs` combines them with `sessions::running` and the
open cards into an `Activity`. The deck asks for one through a new `Backend::activity` on a
background thread, like stats. The pi side reports its state through a new CLI command.

**Tech Stack:** Rust 2024 (ratatui, crossterm, serde_json, chrono), existing test layout
(`#[cfg(test)]` modules, `tests/cli.sh`, `tests/e2e.sh`); TypeScript (`node --test`) for the
dotfiles pi extensions.

**Spec:** `docs/superpowers/specs/2026-11-16-activity-view-design.md`. Read it first.

## Global Constraints

- CI must stay green on macOS and Linux: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`, `tests/e2e.sh`.
- Nothing may block the UI thread: every scan runs through `Runner::background` (or a thread
  like `LoadStats`).
- Missing sources degrade quietly: no `claude` binary, no `~/.claude/projects`, no pi-sub folder
  all mean "fewer rows", never an error toast.
- Paths are overridable: `$CLAUDE_CONFIG_DIR/projects` (default `~/.claude/projects`), `$PI_SUB_DIR`
  (default `~/.local/state/pi-sub`).
- When the state can't be told, the answer is `Unknown`, never `Idle`.
- Transcript reads are tail-only, capped at `TAIL_CAP = 2 MiB` per file.
- Constants: `AGENT_STALE = 10 min` (silent running agent → Unknown), `AGENT_GONE = 2 h` (silent
  unfinished agent → dropped as crashed; *plan decision*: without it a crash days ago would keep
  its session Unknown for good), screen refresh `3 s`, header refresh `5 s`.
- Spinner frames: `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, frame = `(now_ms / 100) % 10`.

## Review Focus

1. A transcript of hundreds of MB: the scan must read at most `TAIL_CAP` of it and stay fast
   (Task 2 test with a 5 MB file).
2. `claude agents --json` failing or absent: Claude sessions known only from the registry show as
   Unknown, the deck works (Task 4 test).
3. An unfinished subagent that crashed long ago: dropped after `AGENT_GONE`, not Unknown forever
   (Task 3 test).
4. A session after `/clear`: transcript and subagents live under `Session.current`, its cards
   under `Session.owner` (Task 4 test).
5. A tail that starts mid-line or holds a malformed line: those lines are skipped, the rest read
   (Task 2 test).

---

### Task 1: The activity model and the classification rule

**Files:**
- Create: `src/activity.rs` (register in `src/lib.rs`)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)] pub enum Status { Idle, Waiting, Working, Unknown }` (declaration order is the screen's sort order)
  - `#[derive(Debug, Clone, PartialEq, Default)] pub struct Doing { pub text: String, pub at: Option<i64> }` (ms)
  - `#[derive(Debug, Clone, PartialEq)] pub struct AgentActivity { pub label: String, pub status: Status, pub since: Option<i64>, pub doing: Option<Doing> }`
  - `#[derive(Debug, Clone, PartialEq)] pub struct SessionActivity { pub session: Session, pub status: Status, pub since: Option<i64>, pub doing: Option<Doing>, pub agents: Vec<AgentActivity>, pub card: Option<String> }` (`card`: the oldest open question card's id)
  - `#[derive(Debug, Clone, PartialEq, Default)] pub struct Activity { pub sessions: Vec<SessionActivity>, pub others: Vec<AgentActivity>, pub at: i64 }` with `working()`, `waiting()`, `idle()`, `unknown()` → `usize`, and `sort(&mut self)` (status, then `session.label()` lowercased)
  - `pub struct Signals<'a> { pub reported: &'a str, pub card: bool, pub background_jobs: usize, pub agents: &'a [AgentActivity] }`
  - `pub fn classify(s: &Signals) -> Status`

- [ ] **Step 1: Write the failing tests**

```rust
fn sig<'a>(reported: &'a str, agents: &'a [AgentActivity]) -> Signals<'a> {
    Signals { reported, card: false, background_jobs: 0, agents }
}
#[test] fn an_open_card_wins_over_working() { assert_eq!(classify(&Signals { card: true, ..sig("working", &[]) }), Status::Waiting); }
#[test] fn needs_input_is_waiting() { assert_eq!(classify(&sig("needs input", &[])), Status::Waiting); }
#[test] fn a_running_agent_makes_an_idle_session_working() { /* agents = [Working] */ assert_eq!(classify(&sig("idle", &a)), Status::Working); }
#[test] fn a_background_job_makes_it_working() { assert_eq!(classify(&Signals { background_jobs: 1, ..sig("idle", &[]) }), Status::Working); }
#[test] fn an_unknown_agent_makes_an_idle_session_unknown() { /* agents = [Unknown] */ assert_eq!(classify(&sig("idle", &a)), Status::Unknown); }
#[test] fn idle_is_idle_and_anything_else_unknown() { assert_eq!(classify(&sig("idle", &[])), Status::Idle); assert_eq!(classify(&sig("", &[])), Status::Unknown); assert_eq!(classify(&sig("starting", &[])), Status::Unknown); }
#[test] fn counts_include_other_agents_and_sort_puts_idle_first() { /* sessions Working "b", Idle "a", Waiting "c", Unknown "d"; others [Working, Unknown] → working()==2, waiting()==1, idle()==1, unknown()==2; after sort labels == ["a","c","b","d"] */ }
```

- [ ] **Step 2: Run** `cargo test activity::` — expect compile failure (module missing).
- [ ] **Step 3: Implement.** Rule order in `classify`: `card || reported == "needs input"` → Waiting; `reported == "working" || background_jobs > 0 || any agent Working` → Working; any agent Unknown → Unknown; `reported == "idle"` → Idle; else Unknown. `working()` counts sessions plus `others`; `unknown()` likewise; `waiting()`/`idle()` sessions only.
- [ ] **Step 4: Run** `cargo test activity::` — PASS; `cargo clippy --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** `feat(activity): the session states and how they are told`

### Task 2: Reading a Claude transcript's tail

**Files:**
- Create: `src/transcript.rs` (register in `src/lib.rs`)

**Interfaces:**
- Consumes: `activity::Doing`
- Produces:
  - `pub const TAIL_CAP: u64 = 2 * 1024 * 1024;`
  - `pub fn find(projects: &Path, session_id: &str) -> Option<PathBuf>` — `<projects>/*/<session_id>.jsonl`
  - `pub fn tail(path: &Path, cap: u64) -> Vec<serde_json::Value>` — start with the last 256 KiB, double until the window holds an `"type":"assistant"` record or reaches `cap` or the file start; drop the first line unless the window starts at byte 0; skip lines that don't parse
  - `pub fn doing(records: &[Value]) -> Option<Doing>` — last `assistant` record: its last `tool_use` as `"<name>: <detail>"`, detail = first present of `input.description`, `input.file_path`, first line of `input.command`, `input.prompt`, `input.pattern`, else just `<name>`; no tool_use → first non-empty text line; cut to 80 chars with `…`; `at` = its `timestamp` in ms
  - `pub fn last_at(records: &[Value]) -> Option<i64>` — newest `timestamp`
  - `pub fn turn_start(records: &[Value]) -> Option<i64>` — timestamp of the last `user` record whose content is a string or holds a `text` item (a prompt, not a `tool_result`)
  - `pub fn open_background(records: &[Value]) -> usize` — ids started minus ids finished. Started: text in `tool_result` items matching `moved to the background as (?:task )?(\w+)`, `running in background with ID: (\w+)`, or `agentId: (\w+)`. Finished: user string content with `<task-id>(\w+)</task-id>` and `<status>(completed|failed|killed|stopped)</status>`. No regex crate: use `str::find` and take the `\w+` run after the marker.

- [ ] **Step 1: Write the failing tests**, with jsonl written to a `tempfile`/temp dir (as other modules' tests do):
  - `doing_names_the_last_tool_call`: records `[assistant tool_use Bash{description:"run the tests"}]` → `Doing { text: "Bash: run the tests", at: Some(<ms of its timestamp>) }`
  - `doing_falls_back_to_text`: assistant text `"\nPlan saved.\nmore"` → `"Plan saved."`
  - `open_background_counts_unfinished_jobs`: two starts (`moved to the background as task k1`, `running in background with ID: b2`), one `<task-id>k1</task-id><status>completed</status>` → `1`
  - `turn_start_ignores_tool_results`: user prompt at t1, assistant, user tool_result at t2 → `Some(t1)`
  - `tail_skips_a_cut_first_line_and_bad_lines`: a 600 KB file whose window starts mid-line, plus one `{oops` line → every returned record parses, the last record is the file's last
  - `tail_reads_at_most_the_cap` (Review Focus 1): 5 MB of `user` records then one assistant at the end → returns within the cap and `doing` finds the assistant; a 5 MB file with no assistant returns ≤ `TAIL_CAP` bytes' worth of records
  - `find_looks_in_every_project_folder`
- [ ] **Step 2: Run** `cargo test transcript::` — fails to compile.
- [ ] **Step 3: Implement** with `File::seek(SeekFrom::End(-n))`.
- [ ] **Step 4: Run** `cargo test transcript::` — PASS; clippy clean.
- [ ] **Step 5: Commit** `feat(activity): read what a Claude session is doing from its transcript tail`

### Task 3: Subagents (Claude and pi-sub)

**Files:**
- Create: `src/agents.rs` (register in `src/lib.rs`)

**Interfaces:**
- Consumes: `transcript::{tail, doing}`, `activity::{AgentActivity, Status}`
- Produces:
  - `pub const AGENT_STALE: i64 = 10 * 60_000; pub const AGENT_GONE: i64 = 2 * 3_600_000;`
  - `pub fn claude(session_dir: &Path, now: i64, detail: bool) -> Vec<AgentActivity>` — `session_dir` is `<project>/<sessionId>`; reads `subagents/agent-*.meta.json`; skips `"finished": true`; age = now − mtime of the matching `.jsonl` (or of the meta if none): `> AGENT_GONE` skipped, `> AGENT_STALE` Unknown, else Working; `label` = `description`, else `agentType`, else the file stem; `since` = meta mtime; `doing` only when `detail` (tail with a 256 KiB cap)
  - `pub fn pi_sub(root: &Path, now: i64) -> Vec<(Option<u32>, AgentActivity)>` — each `<root>/<id>/` with `meta.json` and no `exit.json`; parent = `meta.parent_pid`; label = `label`, else `role`, else the id; age from `events.jsonl` mtime (else meta) with the same two cut-offs; no `doing`

- [ ] **Step 1: Write the failing tests** (temp dirs, `filetime`-free: set mtimes with `File::set_modified`):
  - `a_finished_subagent_is_left_out`
  - `a_running_subagent_is_working_with_its_description`
  - `a_silent_one_is_unknown_after_ten_minutes` (mtime now−11 min)
  - `a_crashed_one_is_dropped_after_two_hours` (Review Focus 3; mtime now−3 h, no `finished`)
  - `pi_sub_skips_exited_agents_and_reads_the_parent_pid`: one with `exit.json`, one with `"parent_pid": 4242` → `[(Some(4242), label …)]`; one without the field → parent `None`
- [ ] **Step 2: Run** `cargo test agents::` — fails.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test agents::` — PASS; clippy clean.
- [ ] **Step 5: Commit** `feat(activity): running subagents of Claude and pi sessions`

### Task 4: The scan

**Files:**
- Modify: `src/activity.rs`
- Modify: `src/sessions.rs` (`Session` gains `#[serde(default)] pub since: i64`)

**Interfaces:**
- Consumes: Tasks 1-3; `sessions::Session`; `queue::Decision` (fields `id`, `session`, `source`, `asked`, and `is_answer()`; check the exact names in `src/queue.rs`)
- Produces:
  - `pub struct Sources { pub claude_projects: PathBuf, pub pi_sub: PathBuf }` with `pub fn from_env() -> Self`
  - `pub fn card_for(s: &Session, cards: &[Decision], all: &[Session]) -> Option<String>` — open question cards (not answer cards) with `session == s.owner`; or, for a card with no `session`, `source` starting with `"<client> · <dir_name>"` when exactly one session in `all` has that prefix; the oldest by `asked`
  - `pub fn scan(src: &Sources, sessions: Vec<Session>, cards: &[Decision], now: i64, detail: bool) -> Activity`

For each session: the Claude id for transcript and subagents is `current` if set, else `owner` (Review Focus 4); `reported` = `Session.state`; `background_jobs` and `doing` from the transcript (only Claude sessions; `doing` only when `detail`); agents = `agents::claude` plus pi-sub entries whose parent equals `session.pid`; pi-sub entries with no matching parent go to `others`. `since`: Waiting → the card's `asked` in ms; Working → `turn_start` (pi: `Session.since`); Idle → `last_at` (pi: `Session.since`); Unknown → None. `activity.sort()`, `at = now`.

- [ ] **Step 1: Write the failing tests** (temp `Sources`, hand-built `Session`s and `Decision`s):
  - `a_session_with_an_open_card_is_waiting_and_names_it` (two cards, the older id wins)
  - `an_old_card_without_a_session_matches_by_source_only_when_unambiguous`
  - `a_cleared_session_reads_its_current_transcript_but_owner_cards` (Review Focus 4)
  - `registry_only_claude_session_is_unknown` (Review Focus 2: `state` empty, no `claude agents`)
  - `orphan_pi_sub_agents_go_to_others`
  - `doing_is_left_out_without_detail`
- [ ] **Step 2: Run** `cargo test activity::` — new tests fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test` — all PASS.
- [ ] **Step 5: Commit** `feat(activity): scan sessions, cards and agents into one picture`

### Task 5: Question cards carry their session

**Files:**
- Modify: `src/store.rs` (`New` gains `pub session: String`; `add` writes `session: <id>` in the front matter when non-empty, the way `add_answer` does)
- Modify: `src/mcp.rs` (the `ask_user` path sets `session: self.owner.clone()`)
- Modify: `src/cli.rs` (`decisions add` gains `--session ID`, optional)

**Interfaces:**
- Produces: question cards whose `Decision.session` is the asking session's owner id. Reuse of an existing card (same asker, question, options, ticket) is unchanged.

- [ ] **Step 1: Write the failing tests:** in `store.rs`, `add_with_a_session_stamps_it` (`show` contains `session: abc`) and `add_without_one_writes_no_session_line`; in `tests/cli.sh`, `decide add … --session s1` then `decide list --json` has `"session":"s1"`.
- [ ] **Step 2: Run** `cargo test store:: && tests/cli.sh` — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test && tests/cli.sh` — PASS.
- [ ] **Step 5: Commit** `feat(mcp): question cards name the session that asked`

### Task 6: `Backend::activity`, live and demo

**Files:**
- Modify: `src/queue.rs` (trait), `src/live.rs`, `src/demo.rs`

**Interfaces:**
- Produces: `fn activity(&self, detail: bool) -> Result<Activity, String>` on `Backend`, default `Ok(Activity::default())`. Live: `activity::scan(&Sources::from_env(), sessions::running(&self.dir), &<open cards from self.store>, now_ms, detail)`. Demo: fixed data matching the spec's demo line (one idle, one waiting on demo card `D-1`, two working with one subagent, one unknown pi session), `at = now`.

- [ ] **Step 1: Write the failing test** in `demo.rs`: `demo_activity_has_every_state` (`working()==2`, `waiting()==1`, `idle()==1`, `unknown()==1`, the waiting session's `card == Some("D-1")`).
- [ ] **Step 2: Run** `cargo test demo::` — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test` — PASS.
- [ ] **Step 5: Commit** `feat(activity): the deck's backends report activity`

### Task 7: Deck state, keys and refresh

**Files:**
- Modify: `src/app.rs`, `src/tui.rs`, `src/main.rs`/`src/cli.rs` (`--activity` flag)

**Interfaces:**
- Consumes: `Backend::activity`
- Produces:
  - `State`: `pub activity: Activity` (last scan, drives the header), `pub activity_open: bool`, `pub activity_cur: usize`, `pub activity_asked: i64`
  - `Event::ActivityLoaded(Result<Box<Activity>, String>)`; `Command::LoadActivity { detail: bool }`
  - `pub fn open_activity(s: &mut State, out: &mut Vec<Command>)` (for `--activity`)
  - `State::spinning(&self) -> bool` = `self.activity.working() > 0`
  - `tui::run(cfg, demo, prompt, activity: bool)`

Rules:
- `tick`: when `now - activity_asked >= 3000` (screen open) or `>= 5000` (closed), push `LoadActivity { detail: activity_open }` and set `activity_asked = now`. The Runner drops a `LoadActivity` while one is in flight.
- `ActivityLoaded(Ok)` replaces `activity`, clamps `activity_cur`; `Err` keeps the old one quietly (no toast; Global Constraints).
- `w` (outside input/help/stats) opens: `activity_open = true`, `activity_cur = 0`, push `LoadActivity { detail: true }`. While open: Esc, `w`, `q` close; `j`/`k`/↓/↑ move over `activity.sessions`; Tab does nothing.
- ⏎ on a row: Waiting with `card: Some(id)` → close the screen and `go` to that card's index in `queue` (toast `card not in the deck` if absent); Waiting with no card → toast `answer it in the session`; otherwise, if the prompt holds a draft (`text` or `images` non-empty) aimed at a different session → toast `draft is for <label>: send or clear it first`; else set `prompt.target = Some(owner)` and `open_prompt`. The prompt line takes keys before the screen (existing order in `key`), so Esc or a send lands back on the screen.
- `tui`: while `s.spinning()`, the frame is at most 100 ms and an `Event::Tick` is pushed each frame (so the header spinner turns); the `LoadActivity` arm is `self.background(move |d| Event::ActivityLoaded(d.activity(detail).map(Box::new)))` plus an in-flight flag cleared on `ActivityLoaded`.

- [ ] **Step 1: Write the failing tests** in `app.rs`, in the style of `s_opens_the_stats_screen_and_esc_s_or_q_close_it`:
  - `w_opens_the_activity_screen_and_esc_w_q_close_it`
  - `tick_asks_for_activity_every_five_seconds_or_three_when_open`
  - `enter_on_a_waiting_row_brings_its_card_to_the_front`
  - `enter_on_an_idle_row_aims_the_prompt_at_it`
  - `enter_keeps_a_draft_aimed_elsewhere` (toast text exactly `draft is for main: send or clear it first`)
  - `a_failed_scan_keeps_the_last_picture`
- [ ] **Step 2: Run** `cargo test app::` — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test` — PASS.
- [ ] **Step 5: Commit** `feat(deck): w opens the activity screen; activity refreshes in the background`

### Task 8: Drawing: the screen and the header spinner

**Files:**
- Modify: `src/ui.rs`, `src/theme.rs` (`Icons` gains `working`: text `⟳`, nerd `\u{f110}`; `waiting_on_you` reuses the hand icon), help text (`?`) lists `w`

**Interfaces:**
- Consumes: `State.activity`, `activity_open`, `activity_cur`
- Produces: `fn activity_screen(f: &mut Frame, area: Rect, s: &State)`; `header` appends, in this order and only when non-zero: `· <frame> N working`, `· <hand> N`, `· ? N` (dim)

Layout per the spec's sketch: title `activity · N working · N waiting on you · N idle · N unknown` and `refreshed Ns ago`; one row per session: state mark (`○` idle, hand waiting, spinner frame working, `?` unknown), label in the session's card colour if `colors` gave it one, state word + duration (`14m`, `3h`), cwd shortened with `~`, `doing` text (for Waiting: `D-n: <question>` if the card is in the queue); agent rows indented with `└`; an "other agents" block when `others` is non-empty; footer `j/k move · ⏎ give it a task · esc back` (on a waiting row: `⏎ answer its card`). Only the idle state word uses the accent colour.

- [ ] **Step 1: Write the failing tests** with ratatui's `TestBackend` (as existing `ui.rs` tests do, if any; else render to a `Buffer` and join rows):
  - `header_shows_spinner_and_counts_only_when_nonzero` (working 2, waiting 1 → contains `2 working` and the hand + `1`; all idle → neither)
  - `activity_screen_lists_idle_first_with_agents_indented`
  - `a_waiting_row_shows_its_card`
- [ ] **Step 2: Run** `cargo test ui::` — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check` — clean. Then by hand: `cargo run -- --demo`, press `w`, check the sketch's look; esc back; header spinner turns.
- [ ] **Step 5: Commit** `feat(deck): draw the activity screen and the header spinner`

### Task 9: The tmux badge and `--activity` end to end

**Files:**
- Modify: `src/cli.rs` (`count --badge`)
- Modify: `tests/e2e.sh`, `tests/cli.sh`

**Interfaces:**
- Produces: `count --badge` prints `<working icon> N` first when `activity(false).working() > 0`, then the existing parts; nothing new when zero. `DECISIONS_ACTIVITY=0` turns the segment off (cheap opt-out for slow machines).

- [ ] **Step 1: Write the failing tests:** `tests/cli.sh`: with `CLAUDE_CONFIG_DIR` and `PI_SUB_DIR` pointing at a temp tree holding one running pi-sub agent → `count --badge` starts with `⟳ 1`; with `DECISIONS_ACTIVITY=0` it doesn't. `tests/e2e.sh`: `--demo`, key `w` shows `activity ·`, Esc returns to the deck.
- [ ] **Step 2: Run** both — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `tests/cli.sh && tests/e2e.sh && cargo test` — PASS.
- [ ] **Step 5: Commit** `feat(cli): the badge shows when agents are working`

### Task 10: pi sessions report their state

**Files:**
- Modify: `src/sessions.rs` (`pub fn set_state(dir: &Path, pid: u32, state: &str, now: i64) -> Result<(), String>` under a lock file `<dir>/sessions/.lock`; refuses a state other than `working`/`idle`; Err when no file for `pid`)
- Modify: `src/cli.rs` (`decisions session-state --pid PID working|idle`, exit 1 on Err)
- Modify: `src/sessions.rs` `merge`: a non-Claude session keeps its registry `state` and `since`

- [ ] **Step 1: Write the failing tests:** `set_state_rewrites_only_state_and_since`; `set_state_refuses_unknown_pid_and_state`; `merge_keeps_a_pi_sessions_state`; `tests/cli.sh`: register a fake session file, `session-state --pid … working`, `sessions` output shows `working`.
- [ ] **Step 2: Run** — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test && tests/cli.sh` — PASS.
- [ ] **Step 5: Commit** `feat(sessions): a harness can report working or idle`

### Task 11: Dotfiles: pi reports state; pi-sub records its parent

Work in a dotfiles worktree (`wt add dotfiles activity-view`).

**Files:**
- Modify: `pi/extensions/daily/index.ts` (in the existing `agent_start`/`agent_end` handlers, not in `PI_SUBAGENT` processes: run `decide session-state --pid ${process.pid} working|idle` detached, errors ignored), test in `index.test.ts`
- Modify: `pi/extensions/pi-sub/state.ts` (`Meta` gains `parent_pid?: number`, set to `process.pid` where the meta is first written), test in `state.test.ts`

- [ ] **Step 1: Write the failing tests:** daily: `agent_start` and `agent_end` spawn `decide` with `["session-state","--pid",String(process.pid),"working"]` / `"idle"` (inject the spawner the way existing tests stub commands); none under `PI_SUBAGENT`. pi-sub: a new agent's `meta.json` has `parent_pid === process.pid`.
- [ ] **Step 2: Run** `cd pi/extensions/daily && npm test` and `cd pi/extensions/pi-sub && npm test` — fail.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** both — PASS.
- [ ] **Step 5: Commit** (dotfiles) `feat(pi): report working/idle to the decisions deck; pi-sub records its parent`

### Task 12: Docs, tmux key, rollout

**Files:**
- Modify (decisions): `README.md` (keys: `w`; flags: `--activity`; `count --badge` segment; `session-state`), `docs/spec.md` (an "Activity" section pointing at the design doc)
- Modify (dotfiles worktree from Task 11): `tmux/.tmux.conf` adds `` bind-key W display-popup -E -B -w 90% -h 90% "$HOME/.local/bin/decisions --activity" ``
- Modify (wiki, through `wiki-commit`): `~/wiki/work/system/decisions-app.md` (Moving parts: `activity.rs`, `transcript.rs`, `agents.rs`, `session-state`; Why: D-943/944/945), `~/wiki/tools/terminal.md` (`` ` W``, `w`, the badge's ⟳)

- [ ] **Step 1:** Write the docs and the binding.
- [ ] **Step 2:** `cargo test && tests/cli.sh && tests/e2e.sh`; in dotfiles the pi tests.
- [ ] **Step 3:** Commit both repos (decisions commit carries no `Docs:` opt-out; the wiki pages are updated).
- [ ] **Step 4:** Rollout, after the user's go: merge the decisions branch, `./install.sh` (rebuilds the binary), `dot-land` the dotfiles branch, `tmux source ~/.tmux.conf`; restart pi sessions to pick up the extension.
