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
  anything is running. Every session is resumable later via `claude --resume <id>`. Quitting
  claude itself (`/exit`, a clean exit) closes its session; a crash or a timeout kill leaves it
  stopped in the list to reopen.
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
  needs-input sort up). Project rows show status counts. `s` cycles the order to projects and
  sessions by name A→Z, then Z→A (natural order, case-insensitive), then back; the panel title
  shows a name sort and it persists.
- Right: the focused session's live terminal.
- Separator is mouse-draggable; widths persist.

## Keys (all ctrl/alt — no ⌘)

**Global keys always win**, even while typing inside a claude pane. Everything else passes through.

| Key      | Action                                    |
|----------|-------------------------------------------|
| ctrl+p   | Switcher                                  |
| ctrl+j   | New prompt                                |
| ctrl+g   | Diff viewer for focused session's tree    |
| ctrl+k   | Commit the focused session's changes      |
| ctrl+t   | Terminal in the focused session's tree    |
| ctrl+e   | Editor for the focused session's tree     |
| ctrl+y   | Memory usage                              |
| ctrl+n   | Agent overview                            |
| ctrl+s   | Settings                                  |
| ctrl+/   | Keyboard shortcuts                        |
| alt+↑/↓  | Prev/next session in the agent list      |
| alt+←/→  | Focus agent list / pane                   |

All rebindable in settings. Note: these keys are then unavailable to claude itself
(e.g. ctrl+p history-prev, ctrl+j newline, ctrl+k kill-line, ctrl+t task list, ctrl+y yank,
ctrl+n history-next, ctrl+e end-of-line).

## Switcher (ctrl+p)

One fuzzy list, sectioned:
1. **Sessions** — all projects, attention order (needs input → working → unseen → rest)
2. **Projects** — jump to project (its most recent session)
3. **Commands** — new worktree, merge or discard the current worktree, clean up unused worktrees,
   remove project, change default model, open settings, …
4. **Add folder** — fuzzy directory finder (rooted at configurable dirs, e.g. `~/Dev`)

## New project

`N` in the agent list or **New project** in the switcher. A small modal takes a folder name,
created under the first project root (`~/Dev`); a relative path nests under it and `~/…` or `/…`
goes anywhere, creating missing parents (shown before you commit). The target is validated live —
invalid names and existing paths show why in red and enter does nothing. `tab` toggles `git init`,
and the choice is remembered. The new folder is added as a project and the new prompt opens on it.

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

## Finishing a worktree

`m` on a session (or **Merge worktree** in the switcher) merges the worktree's branch into the
main checkout's current branch, stops and archives the worktree's sessions, then deletes the
worktree and the branch. Refused if the worktree has uncommitted changes; a conflicted merge is
aborted. **Discard worktree** force-deletes both without merging, after a confirm.

`W` in the agent list (or **Clean up unused worktrees** in the switcher) force-deletes every linked
worktree, across all projects, that has no running session or shell, along with its branch. The
confirm names them and counts any with uncommitted changes; their stopped sessions are archived.

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

## Commit (ctrl+k)

The diff viewer in commit mode: each file in the tree gets a checkbox (all checked to start; a
folder's toggles everything under it) and a message box spans the bottom. Enter commits exactly the
checked files' working-tree state — new, modified, deleted and renamed — and leaves everything else
out, including changes staged earlier, which stay staged. The commit runs in the background so slow
hooks don't freeze the UI; a failure shows git's output and keeps the message. The viewer then
reloads on what's left so a changeset can be split into several commits, and closes once it's clean.

## Terminal (ctrl+t)

A login shell (`$SHELL -l`) in the focused session's checkout (else the selected project), in a
nearly-full-screen overlay. Keys go to the shell except ctrl+t, which hides the overlay; the shell
keeps running hidden (dev servers, watchers) and ctrl+t shows the same one again. A checkout can
have several shells, shown as tabs in the overlay's top border and named after what's running in
the foreground (`zsh`, `node`, ...): alt+t starts another, the prev/next-session keys (alt+↑/↓)
step through them, and alt+1–9 jumps to one. ctrl+t brings back the tab used last. Exiting a shell
(`exit`, ctrl+d) closes its tab and shows a neighbor, or closes the overlay with the last one.
Dragging selects and copies, as in the pane; typing clears the selection.
Quitting maximus counts running shells in its confirm; merging or discarding a worktree stops its
shells.

## Editor (ctrl+e)

A full-screen view of the focused session's checkout (else the selected project's), Cursor-style:

```
╭ Files ───────╮╭ src/app.rs ● ───────────────╮╭ maximus · opus · main · working ─╮
│ ▾ src      ● ││ app.rs ● ×  ui.rs ×         ││                                  │
│   app.rs ● M ││  12 fn main() {             ││   (live claude PTY)              │
│   ui.rs      ││  13     run();              ││                                  │
│ Cargo.toml   ││  14 }                       ││                                  │
╰──────────────╯╰─────────────────────────────╯╰──────────────────────────────────╯
 ^s save  ^z ^y undo/redo  ^f find  ^c ^x ^v copy/cut/paste  ⌥←⌥→ panels  esc files  ^e close
```

- **Files**: the checkout's files as a collapsible tree — what git tracks or would track, so
  ignored files stay out (a plain walk outside a repository). Changed files are coloured with
  their status letter and folders holding changes get a dot. Files with unsaved edits, and the
  folders holding them, get a `●`. Re-read every few seconds, so files claude creates appear.
  `/` filters by fuzzy path match.
- **Tabs**: each open file has a tab above the text, named by its file name (plus enough folders
  to tell apart tabs with the same name), with `●` while it has unsaved edits and always a `×`
  that closes it; a tab whose file changed on disk under unsaved edits is yellow. Enter or a
  click in the list opens a file in a new tab beside the current one, or shows its tab if it's
  open. Space opens a preview (italic) that the next preview replaces, until it's edited, saved
  or opened for real. ctrl+PgUp/PgDn cycle tabs, alt+1–9 jump to one, ctrl+w or a middle click
  closes one (asking to save, discard or cancel when it has unsaved edits). Showing a tab selects
  its file in the list. Tabs scroll to keep the shown one in view.
- **File**: the shown file, syntax highlighted (re-highlighted only from the edited line) and
  editable — typing, auto-indent, tab/shift+tab indent, shift+arrow and mouse selection,
  copy/cut/paste, undo/redo, find. ctrl+s saves, keeping the file's line endings. An open file
  that changes on disk reloads in place when it has no unsaved edits, so claude's edits show live;
  with unsaved edits the title says it changed on disk instead. Binary, huge and non-UTF-8 files
  aren't opened for editing. Beside the line numbers, a bar marks lines changed since HEAD as
  in VS Code — green for added, blue for modified — and a red tick where lines were deleted;
  it follows edits as they're typed. Long lines wrap to the panel's width as in VS Code: between words
  where they can (wide CJK characters break anywhere), whitespace hanging off the row's end,
  and rows after the first indented like the line, which is numbered once. Up/down, page keys,
  clicks and the wheel move by wrapped rows; home/end go to the row's start or end first, then
  the line's. alt+z turns wrapping off or on for the shown file. Every file starts wrapped but
  minified code (`.min.js`, `.min.mjs`, `.min.css`), source maps and tables (`.csv`, `.tsv`),
  whose lines read better whole.
- **Claude**: the session's live pane, resized to the panel and as wide as the file list unless
  dragged; keys and the mouse behave as in the main pane, esc included. ctrl+l (rebindable) jumps
  to it from the list or the file and back again. With no session in the checkout, focusing the
  panel (ctrl+l, alt+→ or a click) starts claude there with no prompt, so there's always
  somewhere to type; a stopped session resumes. Hidden when the screen is too narrow.

The keys that focus the agent list and the pane (alt+←/→) move one panel left/right. ctrl+e
closes from any panel, as does esc in the file list (esc in the file goes back to the list).
Closing with unsaved changes in any tab asks to save them all, discard them all or keep editing.
The save-changes question is a dialog over the file with clickable Save, Don't save and Cancel
buttons (y, n, esc from the keyboard); it takes every key and click until answered.
The editor remembers its tabs and folders for the next ctrl+e in the same checkout. Panel
separators drag, and their widths persist.

## Memory (ctrl+y)

A small centered modal listing resident memory (RSS from `ps`) for each live claude session and
each terminal shell — each counted with its whole process tree, so tools, MCP servers and dev
servers land on whatever started them — then maximus itself and the total. Refreshes every two
seconds while open. RSS counts shared pages once per process, so totals run a little high.

## Overview (ctrl+n)

A quick-look alternative to the Agents panel (also `o` in the agent list): a centered grid with a
tile per visible session, in attention order. Each tile shows the status dot and name, the project
(and worktree branch), the status word and age; needs-input tiles get a red border and unseen
finished ones blue. A summary line counts each status. Arrows/`hjkl` move, enter or a click opens
the session (resuming a stopped one), esc closes. Rows scroll when there are more tiles than fit.

## Keyboard shortcuts (ctrl+/)

A centered modal listing every shortcut, grouped by where it works: global, agent list, pane,
commit graph, new prompt, diff viewer, commit, terminal, editor and overview. Global keys show the user's
current bindings. Also `?` in the agent list and **Keyboard shortcuts** in the switcher. A filter
line at the top narrows the list as you type: each word must appear in the row's key, description
or group title (`ctrl`, `alt`, `shift` also match `^`, `⌥`, `⇧`), and empty groups drop out. The
modal keeps its size while filtering. When the list is taller than the screen it scrolls (arrows,
PgUp/PgDn, Home/End, mouse wheel) and the title shows which lines are in view. esc clears the
filter, then closes; ctrl+/ closes. Legacy terminals report ctrl+/ as ctrl+7, which matches too.
A config still holding the old `f1` default moves to ctrl+/ on load.

## Settings (ctrl+s)

Modal with tabs: **General** (default model, effort, permission mode, Chrome via `claude --chrome`, project roots for Add folder) ·
**Sessions** (timeouts) · **Keys** (rebind) · **Notifications** · **Theme** (palette, startup animation).
Stored at `~/.config/maximus/config.toml`; project list + layout in `~/.local/state/maximus/`.

Timeouts (each can be off):
- **Idle kill** — stop a claude process after N min idle. The session stays in the list as
  expired (never archived), and its pane says so: enter or `r` reopens it.
- **Archive finished** — hide stopped sessions after N min (still in switcher); a session whose
  claude is still running stays in the list however long it's idle.
- **Needs-input nag** — re-notify after N min waiting.

## Attention

- Status dots + counts, needs-input sorted to top.
- macOS notification when a session needs input / finishes while you're not looking at it.
  iTerm2 (OSC 9), Ghostty and WezTerm (OSC 777) post it themselves, so clicking it brings the
  terminal back; elsewhere (Terminal.app, inside tmux) it goes through osascript, which macOS
  attributes to Script Editor.
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
