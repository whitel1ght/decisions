# Copyable commands on deck cards: design

Date: 2026-11-23 · Approved: D-1306

## Intent
Cards often ask the user to run a command. In the deck the command is cut off at the card's
edge, and there is no way to copy it, so the user retypes it. They want to read the whole
command and copy it with one key.

## Cause
- `markdown::render` pushes a fenced code line as one `Line`, never wrapped, and the card back
  draws a `Paragraph` without wrap (`src/markdown.rs:21`, `src/ui.rs:952`): a line wider than the
  card is clipped.
- Inline code wraps on spaces with an indent, which breaks a command for copying by selection.
- Telegram is fine: code goes as `<pre>`/`<code>`, which Telegram copies on a tap.

## Goals
- A long fenced code line wraps; continuation rows start with `↪ ` so it reads as one line.
- `c` copies the current card's command to the clipboard. A card with several commands: `c`
  copies the first; the next `c` on the same card copies the next, wrapping around. The toast
  says `copied 1/2: <first 40 chars>`; none: `no command on D-n`.
- Commands are, in order: each fenced block of the Context (its lines joined by `\n`), then
  every inline `` `code` `` span that contains a space, in the Context and in the options.
  Duplicates are kept once.
- Clipboard: `pbcopy` when it exists, else `wl-copy`, else `xclip -selection clipboard`, else an
  OSC 52 escape written to the terminal (tmux forwards it with `set-clipboard on`).
- Agents put a command for the user in its own fenced block: one line in the `ask_user` tool
  description, and one in the Questions section of dotfiles' `claude/CLAUDE.md`.

## Non-goals
- Telegram (already copyable), running a command from the deck, a highlight of the selected
  command on the card (the toast names it).

## Anti-goals
- A copied command that differs from what the agent wrote (wrap markers or indents in it).
- A copy that blocks the deck's event loop.
