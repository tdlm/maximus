# Ideas

Possible features, roughly in priority order within each group. Not commitments.

## Working with agents

- **Reply from the diff.** Select a hunk in the diff viewer and press a key to write "about this
  change: …" into that session's pane, turning the viewer into a lightweight review tool.
- **Run one prompt in several places.** Send the same prompt to N new worktrees, or to several
  projects at once ("bump this dependency everywhere").
- **Queue a follow-up.** Type the next prompt while claude is still working; maximus sends it when
  the `Stop` hook fires.
- **Prompt templates.** Save prompts like "review this branch" or "write tests for the diff" and
  pick them from the new-prompt modal.

## Git

- **Act on changes in the diff viewer.** Stage, revert or discard a hunk or file, and commit with a
  message — or send `/commit` to the session's claude instead of building a commit editor.
- **Open a PR.** Run `gh pr create` from the worktree, then show the PR and CI status on the
  session row.
- **Rebase option when finishing a worktree**, as an alternative to merging.

## Visibility

- **Activity line per session.** Show the last tool and its target (`Edit src/app.rs`,
  `Bash cargo test`) from the `PreToolUse` hook, which maximus already receives.
- **Cost and context use.** Read token counts and cost from claude's transcript JSONL (the hook
  payload includes `transcript_path`); show them per session and project, with an optional budget
  warning.
- **Elapsed time.** "working 4m" / "waiting 12m" on each row.
- **Search transcripts.** Fuzzy-search what claude said or did across all sessions, including
  stopped ones, from the switcher.

## Layout and launching

- **Split panes.** Two sessions side by side, or pin one to watch while working in another.
- **Settings per project.** Override model, permission mode or extra `claude` args per project.
- **Run a command per worktree.** A plain shell, dev server or test watcher pane next to a session
  in the same tree.
- **Restore on startup.** Reopen the sessions that were live at quit, via `--resume`.
- **Notification actions.** Clicking a macOS notification focuses that session (needs
  `terminal-notifier` or a small helper).
