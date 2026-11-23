# Copyable Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the deck, long code lines wrap instead of being clipped, and `c` copies the card's command(s) to the clipboard.

**Architecture:** `markdown::render` wraps fenced lines by width with a `↪ ` continuation. A new `src/commands.rs` extracts a card's commands. `app.rs` maps `c` to a new `Command::Copy(String)` and keeps a per-card cycle index. `tui.rs` hands the text to a new `src/clipboard.rs`, which pipes it to the first clipboard tool it finds, or else writes OSC 52.

**Tech Stack:** Rust (ratatui, crossterm), `cargo test`, clippy `-D warnings`, rustfmt.

**Spec:** `docs/superpowers/specs/2026-11-23-copy-commands-design.md` (approved D-1306)

## Global Constraints
- Worktree `~/projects/worktrees/decisions/copy-commands`, branch `copy-commands` from main.
- `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass after every task (CI runs all three).
- The continuation marker is exactly `↪ `. The toast reads exactly `copied {i}/{n}: {first 40 chars, newlines shown as ⏎}` or `no command on {id}`.
- A copied string is byte-for-byte what the agent wrote: no marker, no indent.
- The deck never blocks on the clipboard: copying runs on a thread, and a failure comes back as a toast `copy failed: …`.

## Review Focus
1. A fenced line with no spaces (a long URL or base64) longer than the card must still wrap by character. Test in Task 1.
2. A multi-line fenced block copies as all its lines joined by `\n`, with the fence lines and language tag left out. Test in Task 1.
3. Pressing `c` after switching cards starts again at 1/n for the new card. Test in Task 2.
4. An unterminated fence (an agent forgot the closing ```) still yields the block up to the end. Test in Task 1.
5. Inline code without a space (`D-12`, `main`) is not a command. Test in Task 1.

---

### Task 1: Wrapped code lines and command extraction

**Files:** Modify `src/markdown.rs`; create `src/commands.rs` (add `mod commands;` in `src/main.rs`/`lib.rs`, wherever modules are declared).

**Interfaces:** Produces `pub fn commands(context: &str, options: &[&str]) -> Vec<String>`.

- [ ] **Step 1: Failing tests.** In `markdown.rs`:
  - `long_fenced_lines_wrap_with_a_marker`: `render("```\n" + 60×"a" + "\n```", 30)` gives 3 lines; the first starts with `"  "`, and the next two start with `"  ↪ "`. No line is wider than 30.
  - `long_fenced_word_wraps_by_character` (the same with a 60-char token and no spaces).

  In `commands.rs`:
  - `fenced_blocks_come_first_joined_by_newlines`: the context ``"x\n```bash\nkubectl get pods\n  -n prod\n```\nthen `git push origin main`"`` gives `["kubectl get pods\n  -n prod", "git push origin main"]`.
  - `inline_code_needs_a_space`: `` `D-12` `` and `` `main` `` are ignored.
  - `options_are_scanned_after_the_context`: the option ``"You run `duploctl jit k8s --interactive` and sign in"`` yields the command.
  - `duplicates_are_kept_once`.
  - `unterminated_fence_runs_to_the_end`.
- [ ] **Step 2: Run** `cargo test markdown commands`. Expected: fail (no wrapping, no module).
- [ ] **Step 3: Implement.** In `render`, a fenced line is split into rows of at most `width - 2` display columns (first row prefix `"  "`; later rows `"  ↪ "` with `width - 4`), by `unicode-width`, at char boundaries. `commands` does fences first (the language tag after ``` is dropped), then inline spans: text between single backticks outside fences, kept only when it contains a space.
- [ ] **Step 4: Run** `cargo test`, clippy, fmt. Expected: all pass.
- [ ] **Step 5: Commit** `feat(deck): long code lines wrap; commands() finds a card's commands`.

### Task 2: The `c` key and the clipboard

**Files:** Modify `src/app.rs` (Command enum, key match, State, tests), `src/tui.rs` (run `Command::Copy`), `src/ui.rs` (help line `("c", "copy the card's command (again: the next one)")`), `README.md` (key table row after `o`); create `src/clipboard.rs`.

**Interfaces:**
- Consumes `commands()` from Task 1.
- Produces `Command::Copy(String)`, `Event::Copied(Result<(), String>)`, `State.copy_at: Option<(String /*card id*/, usize)>`, and `clipboard::copy(text: &str) -> Result<(), String>`.

- [ ] **Step 1: Failing tests** in `app.rs` (the same style as `o_opens_the_mr_or_says_there_is_none`):
  - `c_copies_the_command_and_cycles`: on a fixture card with two commands, `c` gives `[Command::Copy(first)]` with the toast `copied 1/2: …`; a second `c` gives the second command with `copied 2/2: …`; a third `c` gives the first again.
  - `c_restarts_on_another_card`: `c`, then `J`, then `c` gives the new card's first command, `1/n`.
  - `c_without_a_command_says_so`: the toast is `no command on D-n` and no command is pushed.
  - `a_failed_copy_is_a_toast`: `update(Event::Copied(Err("x")))` gives the toast `copy failed: x`.

  In `clipboard.rs`: `osc52_encodes_base64` checks that `osc52("hi")` is `"\x1b]52;c;aGk=\x07"`.
- [ ] **Step 2: Run** `cargo test`. Expected: fail.
- [ ] **Step 3: Implement.**
  - `copy(s, out)` takes the current card's `commands(&d.context, &option texts)` and advances `copy_at`.
  - In `tui.rs`, `Command::Copy` runs `clipboard::copy` on a thread through `self.background`-style spawn and sends `Event::Copied`.
  - `clipboard::copy` tries `pbcopy`, `wl-copy`, then `xclip -selection clipboard`. The text goes to the child's stdin and the exit status is checked. When none of them exists, it writes `osc52(text)` to `/dev/tty`.
  - Base64 is a small local encoder; no new crate unless one is already a dependency.
- [ ] **Step 4: Run** `cargo test`, clippy, fmt, and `bash tests/e2e.sh` (it drives the real binary in a pty). Expected: all pass.
- [ ] **Step 5: Commit** `feat(deck): c copies the card's command, again for the next one`.

### Task 3: Agents write commands copyably, docs, install

**Files:** Modify `src/mcp.rs` (the `ask_user` description: add "Put a command the user should run in its own fenced code block, so the deck can copy it with c."), `docs/spec.md` (the key list, if it has one), dotfiles `claude/CLAUDE.md` Questions section (one line: "A command I should run goes in its own fenced block in the card's Context, never only inside an option or a sentence; the deck copies it with `c`.") plus `claude/CLAUDE.test.sh`, wiki `work/system/decisions-app.md` and `tools/workflows.md` (the deck keys).

- [ ] **Step 1: Failing test:** in dotfiles `claude/CLAUDE.test.sh`, a `has` for "own fenced block" in the Questions section. Run it and watch it fail.
- [ ] **Step 2: Make the edits.** Run `cargo test` (the MCP description is covered by its snapshot/contains test if one exists; else add `ask_user_description_asks_for_fenced_commands`) and `bash claude/CLAUDE.test.sh`.
- [ ] **Step 3: Commit** in decisions (`docs(mcp): ask_user asks for commands in fenced blocks`), in a dotfiles branch `copy-commands` (land it with `dot-land`), and in the wiki with `wiki-commit`.
- [ ] **Step 4: Land.**
  - decisions: ask the user on a card whether the agent merges `copy-commands` into main and pushes (only dotfiles merges are pre-authorised).
  - After the merge, run `~/projects/dotfiles/install.sh`'s `setup_decisions` path (or `cargo install --locked --path ~/projects/decisions --root ~/.local/share/decisions`).
  - Check in the deck that `c` copies (`pbpaste`).
