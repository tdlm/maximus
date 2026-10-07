# maximus

A keyboard-first terminal UI for running many Claude Code sessions across many folders.
See [SPEC.md](SPEC.md) for the design.

Each session is the real `claude` CLI in an embedded terminal pane, so everything Claude Code does
works as usual. maximus adds a cross-project agent list with live status, a `ctrl+p` switcher,
a prompt launcher, a changeset viewer, and notifications.

## Build & run

```sh
cargo build --release
./target/release/maximus            # adds the current git repo as a project on first run
./target/release/maximus ~/Dev/app  # add a specific folder
```

Install onto your PATH with `cargo install --path .` (puts it in `~/.cargo/bin`).

## Keys

Global (work everywhere, even while typing in a claude pane — rebind in Settings → Keys):

| Key        | Action                                   |
|------------|------------------------------------------|
| `ctrl+p`   | Switcher: sessions, projects, commands, add folder |
| `ctrl+l`   | New prompt                               |
| `ctrl+g`   | Diff viewer for the current session's checkout |
| `ctrl+s`   | Settings                                 |
| `alt+↓/↑`  | Next/previous session in attention order (needs input → working → unseen) |
| `alt+←/→`  | Focus agent list / pane                  |

Agent list: `j/k` move · `enter` open (resumes stopped) · `n` new prompt · `w` new prompt in a new
worktree · `r` resume · `x` close session / remove project · `d` diff · `/` switcher · `,` settings · `q` quit.

Pane: everything goes to claude. `shift+PgUp/PgDn` or the mouse wheel scrolls back; drag to select
and copy.

New prompt: `enter` send · `alt/shift+enter` newline · `ctrl+p` project · `ctrl+t` worktree
(main / existing / new) · `ctrl+o` model (←/→ effort) · `tab` rename the new branch.

Diff viewer: `j/k` file · `J/K` hunk · `enter` focus file · `s` split/unified · `]`/`[` next/prev file ·
`h/l` fold · `r` refresh · `esc` back/close.

Mouse: click to select/focus, drag the pane separators, scroll lists, panes and diffs.

## Status

Status comes from Claude Code hooks: maximus starts each `claude` with `--settings` registering
`maximus hook` on `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification` and `Stop`.
The hook forwards a one-line event over a unix socket. Your own hooks keep working.

| Dot | Meaning |
|-----|---------|
| yellow spinner | working |
| red ● | needs your input (permission prompt, question, plan approval) |
| blue ● | finished, not yet seen |
| ○ | idle / finished and seen |
| ◌ | stopped (resumable) |

## Files

- `~/.config/maximus/config.toml` — settings (also editable in-app)
- `~/.local/state/maximus/state.json` — projects, layout, session history
- `~/.local/share/maximus/worktrees/` — worktrees created by maximus (configurable)

Set `MAXIMUS_CONFIG_DIR` / `MAXIMUS_STATE_DIR` to use other locations.

## iTerm2 notes

- `alt+arrow` keys need iTerm to send them as escape sequences. That's the default unless your
  profile uses the "Natural Text Editing" key preset, which remaps them; rebind them in Settings if so.
- Copying uses `pbcopy`. iTerm's native selection still works with `⌥`-drag.
- The badge ("2 waiting") uses iTerm2's `SetBadgeFormat` and does nothing in other terminals.
