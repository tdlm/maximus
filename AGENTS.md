# AGENTS.md

Guidance for coding agents working in this repo. See [SPEC.md](SPEC.md) for the design and
[README.md](README.md) for usage.

## Checks

Run before committing; CI runs the same on macOS and Linux:

```sh
make check   # cargo fmt --check + cargo test
make lint    # cargo clippy --all-targets -- -D warnings
```

`make dev` runs a debug build with config and state isolated in `.dev/`.

## Git conventions

### Commits

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope): description`.

- **Types:** `feat`, `fix`, `refactor`, `perf`, `docs`, `build`, `ci`, `test`, `chore`.
  `feat`/`fix` are for changes users notice; `build` covers Cargo, the Makefile, and dist;
  `ci` covers `.github/workflows`.
- **Scope:** the module or area touched, lowercase — usually the `src/` file name without `.rs`
  (`app`, `ui`, `git`, `diff`, `config`, `settings`, `session`, `hooks`, `keys`, `prompt`,
  `switcher`, ...), or `readme`, `spec`, `make`, `dist`, `cargo`. Omit it for broad changes.
- **Description:** imperative mood, lowercase start, no trailing period; say what the change
  does, not how.
- **Body:** usually present. A short prose paragraph wrapped at ~80 columns explaining what
  changed and why; use bullets only when one commit covers several distinct changes. Skip it
  when the subject says everything.
- **No ticket references.** No `Co-Authored-By`, "Generated with", or other attribution
  trailers.
- One concern per commit, ordered so the log reads as a build-up (data and helpers → state →
  UI → docs). Each commit should build.

Examples from history:

```text
feat(git): read the commit graph and each commit's files
docs(readme): document installing and releasing
build(make): add dist-plan and tag targets
ci: check, lint, and build on macOS and Linux
```

### Branches

The default branch is `main`, and work so far has landed on it directly.

### Releases

Bump `version` in `Cargo.toml`, commit, then `make tag` (see README → Releasing).
