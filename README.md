# maximus

A keyboard-first terminal UI for running many Claude Code sessions across many folders.
See [SPEC.md](SPEC.md) for the design.

Each session is the real `claude` CLI in an embedded terminal pane, so everything Claude Code does
works as usual. maximus adds a cross-project agent list with live status, a `ctrl+p` switcher,
a prompt launcher, a changeset viewer, and notifications.

## Install

Prebuilt binaries for macOS and Linux (Apple Silicon/ARM64 and x86_64):

```sh
brew install tdlm/tap/maximus
```

or

```sh
curl -LsSf https://github.com/tdlm/maximus/releases/latest/download/maximus-installer.sh | sh
```

The curl installer puts `maximus` in `~/.local/bin`; make sure that's on your PATH.

You also need the [`claude` CLI](https://code.claude.com/docs/en/setup) and `git` on your PATH.

| | Homebrew | curl installer |
|---|---|---|
| Upgrade | `brew upgrade maximus` | re-run the install command |
| Uninstall | `brew uninstall maximus` | `rm ~/.local/bin/maximus` |

Settings and state live in `~/.config/maximus` and `~/.local/state/maximus`; delete those too
for a clean uninstall.

## Quick start

```sh
maximus            # run inside a git repo to add it as a project
maximus ~/Dev/app  # or add a specific folder
```

1. `ctrl+p`, type part of a folder name, `enter` to add it (folders come from `~/Dev` by default;
   change it in Settings → General → Folder search roots, or type a path like `~/work/api`).
2. `ctrl+j`, type a task, `enter` — claude starts working in a pane on the right.
3. `alt+↓` jumps to whichever session needs you; `ctrl+g` shows what it changed.

**First run:** sessions are the real `claude` CLI, so if you haven't used it in a terminal before,
the first pane shows claude's own setup (theme, login) and later a "trust this folder" prompt for
each new folder or worktree. Finish those in the pane once; maximus shows the session as idle
until claude starts on your prompt.

**Linux:** everything works the same except desktop notifications (macOS only; the terminal bell
still rings), the badge (iTerm2 only), and copy, which uses OSC 52 and so needs a terminal that
allows clipboard access.

## Build from source

Needs Rust ([rustup](https://rustup.rs), or `brew install rustup && rustup default stable`).
Run `make` to list targets:

```sh
make run      # build and run the release binary
make dev      # debug build with config/state isolated in .dev/
make install  # install to ~/.cargo/bin
```

## Releasing

Releases are built by [dist](https://opensource.axo.dev/cargo-dist/) in GitHub Actions
(`.github/workflows/release.yml`). Bump `version` in `Cargo.toml`, commit, then:

```sh
make tag      # tags v<version> and pushes it
```

The workflow builds all four targets, creates a GitHub Release with the binaries and an install
script, and pushes an updated formula to [tdlm/homebrew-tap](https://github.com/tdlm/homebrew-tap)
(needs the `HOMEBREW_TAP_TOKEN` repo secret). `make dist-plan` previews a release. After changing
`dist-workspace.toml`, run `dist generate` to regenerate the workflow.

## Keys

Global (work everywhere, even while typing in a claude pane — rebind in Settings → Keys):

| Key        | Action                                   |
|------------|------------------------------------------|
| `ctrl+p`   | Switcher: sessions, projects, commands, add folder |
| `ctrl+j`   | New prompt                               |
| `ctrl+g`   | Diff viewer for the current session's checkout |
| `ctrl+k`   | Commit the current session's changes     |
| `ctrl+t`   | Terminal in the current session's checkout (again to hide) |
| `ctrl+y`   | Memory used by claude sessions, shells and maximus |
| `ctrl+n`   | Overview of every agent as tiles         |
| `ctrl+s`   | Settings                                 |
| `f1`       | All keyboard shortcuts, in a scrollable list |
| `alt+↓/↑`  | Next/previous session in agent-list order, wrapping around; in the left column, move between Agents and Graph |
| `alt+←/→`  | Focus agent list / pane                  |
| `ctrl+q`   | Show/hide the commit graph               |

Agent list: `j/k` move · `enter` open (resumes stopped) · `n` new prompt · `w` new prompt in a new
worktree · `N` new project · `o` overview · `?` keyboard shortcuts · `r` resume · `e` rename session · `x` close session / remove project · `m` merge worktree · `d` diff · `t` graph · `s` sort (attention → name A→Z → Z→A) · `/` switcher · `,` settings · `q` quit.

Graph (commits on the current session's checkout): the selected commit expands to show its changed
files · `j/k` move through commits and files · a selected file's diff shows on the right
(`PgUp/PgDn` scroll, `h/l` pan, `s` split/unified) · `y` copy hash · `esc` collapse the commit,
again to go back to Agents (or `alt+↑`) · `t` hide.

Finishing a worktree: `m` on a session in a worktree merges its branch into the branch the main
checkout is on, stops the worktree's sessions, and deletes the worktree and branch. It refuses if
the worktree has uncommitted changes and aborts the merge on conflicts, leaving everything as it
was. The switcher also has **Discard worktree**, which deletes the worktree and branch without
merging.

Pane: everything goes to claude. `shift+PgUp/PgDn` or the mouse wheel scrolls back; drag to select
and copy.

New project (`N`, or **New project** in the switcher): type a folder name and it's created in your
first project root (`~/Dev` by default); a relative path like `work/app` nests under it, and
`~/…` or `/…` puts it anywhere. Missing parent folders are created too, and the modal says so.
The path is checked as you type — empty names, `.`/`..`, `:`, control characters, leading or
trailing spaces, names over 255 bytes, and paths that already exist are refused with the reason.
`tab` toggles `git init` (remembered). The folder is added as a project and a new prompt opens
in it. Typing a path that doesn't exist into the switcher offers to create it the same way.

New prompt: `enter` send · `alt/shift+enter` newline · `ctrl+p` project · `ctrl+t` worktree
(main / existing / new) · `ctrl+o` model (←/→ effort) · `tab` rename the new branch.

Diff viewer: `j/k` file · `J/K` hunk · `enter` focus file · `s` split/unified · `]`/`[` next/prev file ·
`h/l` fold · `r` refresh · `esc` back/close · `ctrl+k` switch to commit mode.

Commit (`ctrl+k`): the diff viewer with a checkbox on every changed file, all checked, and a message
box below. Type the message and press `enter` to commit just the checked files; `alt/shift+enter`
adds a newline and `tab` moves to the file list, where `space` checks or unchecks a file (or a whole
folder) and `a` toggles all. A file you staged earlier but leave unchecked stays staged and out of
the commit. After a commit the viewer stays open on what's left, so you can split the changes into
several commits; it closes when nothing is left. If a hook rejects the commit, its output shows on
the right and your message is kept.

Terminal (`ctrl+t`): a shell in the current session's checkout (or the selected project), over
the main screen. Everything you type goes to the shell except `ctrl+t`, which hides the terminal and
leaves the shell running, so a dev server or watcher keeps going; `ctrl+t` brings back the same
shell. `exit` or `ctrl+d` ends the shell and closes the terminal. Each checkout gets its own shell.
`shift+PgUp/PgDn` or the mouse wheel scrolls back. Quitting asks first while any shell is running.

Memory (`ctrl+y`): resident memory of each running claude session, each terminal shell, and
maximus itself, with the total. A session or shell counts everything it started (tools, MCP
servers, a dev server), so the total is what maximus is responsible for. It refreshes every two
seconds while open; `esc` or `ctrl+y` closes it.

Overview (`ctrl+n`, or `o` in the agent list): every agent in the Agents list as a tile — name,
project and worktree branch, status (needs input, working, done, idle or stopped) and age — in
attention order, so whatever needs you comes first; tiles needing input get a red border, unseen
finished ones a blue one. A line at the top counts each status. Arrows or `hjkl` move, `enter` or a
click opens the session (resuming it if stopped), `esc` or `ctrl+n` closes.

Keyboard shortcuts (`f1`, or `?` in the agent list): every shortcut in one modal, grouped by where
it works, with global keys showing your current bindings. `j/k`, arrows, `PgUp/PgDn` or the mouse
wheel scroll it when it doesn't fit; `esc` or `f1` closes.

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
