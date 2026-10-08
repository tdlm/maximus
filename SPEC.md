# maximus — spec (v0)

A keyboard-first Rust TUI for running many Claude Code sessions across many project folders,
with a ctrl+p switcher, live status, and a changeset viewer. Primary terminal: iTerm2. Must also
work in any terminal at work (no ⌘ keys, no iTerm-only features required).

## Core model

- **Project**: a folder (usually a git repo) added to maximus. Removing it only forgets it — no files touched.
- **Worktree**: default is the project's main checkout. Optionally a `git worktree`.
- **Session**: a real `claude` CLI process running in a PTY, embedded in a pane (nebula-style).
  Full Claude Code feature parity; maximus does not re-render the conversation.
- **Lifecycle**: sessions are owned by the TUI process (no daemon). Quitting asks to confirm if
  anything is running. Every session is resumable later via `claude --resume <id>`.
- **Status** comes from Claude Code hooks (UserPromptSubmit / PreToolUse / Notification / Stop),
  injected per session via `--settings`, reporting over a unix socket to maximus. No screen-scraping.

| Status            | Dot        |
|-------------------|------------|
| working           | ● yellow   |
| needs input       | ● red      |
| done, unseen      | ● blue     |
| done, seen / idle | ○ dim      |

## Main screen

```
╭ Agents ────────────╮╭ maximus · opus · main ──────────────────╮
│ ● maximus  ●1 ●1   ││                                         │
│   ● needs input    ││   (live claude PTY)                     │
│   ● settings modal ││                                         │
│   ○ fix keybinds   ││                                         │
│ ● api-server       ││                                         │
│ ○ blog             ││                                         │
╰────────────────────╯╰─────────────────────────────────────────╯
 ^p switch  ^j new  ^g diff  ^s settings
```

- Left: sessions grouped by project; needs-input sorts to the top of each group (and projects with
  needs-input sort up). Project rows show status counts.
- Right: the focused session's live terminal.
- Separator is mouse-draggable; widths persist.

## Keys (all ctrl/alt — no ⌘)

**Global keys always win**, even while typing inside a claude pane. Everything else passes through.

| Key      | Action                                    |
|----------|-------------------------------------------|
| ctrl+p   | Switcher                                  |
| ctrl+j   | New prompt                                |
| ctrl+g   | Diff viewer for focused session's tree    |
| ctrl+s   | Settings                                  |
| alt+↑/↓  | Prev/next session (attention order)       |
| alt+←/→  | Focus agent list / pane                   |

All rebindable in settings. Note: these keys are then unavailable to claude itself
(e.g. ctrl+p history-prev, ctrl+j newline). ctrl+k stays with claude (kill-line).

## Switcher (ctrl+p)

One fuzzy list, sectioned:
1. **Sessions** — all projects, attention order (needs input → working → unseen → rest)
2. **Projects** — jump to project (its most recent session)
3. **Commands** — new worktree, remove project, change default model, open settings, …
4. **Add folder** — fuzzy directory finder (rooted at configurable dirs, e.g. `~/Dev`)

## New prompt (ctrl+j)

Centered modal:

```
╭ New prompt ─────────────────────────────╮
│ project maximus ^p · tree main ^t       │
│ model opus ^o · effort high             │
├─────────────────────────────────────────┤
│ › _                                     │
╰ enter send · alt+enter newline · esc ───╯
```

- Defaults to the focused project, main checkout, default model.
- `^t` picks: main checkout (default) · existing worktree · **new worktree** (branch auto-named from
  prompt, editable).
- Enter launches `claude` with the prompt; focus follows the new session.

## Diff viewer (ctrl+g)

Centered, nearly-full-screen modal over the main screen.

```
╭ Changes (4) ─────╮╭ src/settings.tsx ─────────────────────────────────╮
│ ▾ src/           ││  old                    │  new                    │
│   M app.tsx  +3-1││  const x = 1            │  const [open, setOpen]… │
│ ▸ A settings.tsx ││                         │  useHotkey('ctrl+s', …) │
│   D old.ts       ││                         │                         │
│ M package.json   ││                         │                         │
╰──────────────────╯╰───────────────────────────────────────────────────╯
 j/k file · J/K hunk · s unified/split · enter focus file · esc close
```

- Left: collapsible file tree of the worktree's changes (vs HEAD, incl. untracked).
- Right: side-by-side by default, `s` toggles unified. Syntax highlighting.
- Draggable tree/file separator.

## Settings (ctrl+s)

Modal with tabs: **General** (default model, effort, permission mode, project roots for Add folder) ·
**Sessions** (timeouts) · **Keys** (rebind) · **Notifications** · **Theme**.
Stored at `~/.config/maximus/config.toml`; project list + layout in `~/.local/state/maximus/`.

Timeouts (each can be off):
- **Idle kill** — stop a claude process after N min idle (resumable).
- **Archive finished** — hide done sessions after N min (still in switcher).
- **Needs-input nag** — re-notify after N min waiting.

## Attention

- Status dots + counts, needs-input sorted to top.
- macOS notification when a session needs input / finishes while you're not looking at it.
- Terminal bell + iTerm2 badge (`OSC 1337 SetBadgeFormat`, e.g. "2 waiting"); badge is a no-op elsewhere.

## Mouse

Click to select/focus everywhere · drag separators · scroll wheel in lists, panes, diff ·
in-app drag-select + copy (OSC 52 to clipboard) since mouse capture disables native selection.

## Remove project

Via switcher command. If sessions are live: `maximus has 2 running sessions. Stop them and remove? [y/N]`.

## Look

Rounded borders, muted palette, accent on the focused pane. Built-in truecolor themes
(e.g. Catppuccin Mocha, Tokyo Night, Gruvbox, Beardy Blueberry), selectable in settings.

## Stack

Rust · ratatui + crossterm · portable-pty · vt100 (or alacritty_terminal) for pane emulation ·
git2 or `git` CLI for diffs/worktrees · syntect for highlighting · notify-rust / osascript for notifications.
