# Common rules for every Loomward lane

- Host: Windows, shared by several parallel agents. Be frugal: use the launcher's default
  `CARGO_BUILD_JOBS` (3), iterate with `-p <crate>` and only run the whole workspace at the end.
- Your worktree and branch are already created (named in your task). Work ONLY inside it. Run every git command
  as `git -C <worktree>`; use absolute paths (the Bash cwd resets between calls).
- Each worktree builds in its own default `target/` and never sets `CARGO_TARGET_DIR` (issue #601:
  shared target dirs overwrite identical artifact names). Never use the primary checkout or touch its
  gitignored `Resources/` folder.
- Read `AGENTS.md` first (product invariants are bugs if violated), then the docs your task names.
- Regression test first, then the change. New behaviour lands with the test that pins it. Never disable, skip
  or weaken an existing test. Synthetic fixtures only: no real file names or paths in Git.
- Known host flakes: #292 is fixed (#479). Under heavy load a few timing-bound service tests (#530, #533, #538 family)
  can time out and pass alone; re-run those alone before reporting a failure, and report them as pre-existing
  only when evidence establishes that cause.
- Temp: `TMP` and `TEMP` point at `<worktree>/target/lane-tmp` (the sandbox cannot write the user Temp); keep them
  set for every cargo or test command. If cargo reports `.cargo-build-lock` access denied, stop building and say so.
- Proving checks before you finish: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo test` for every crate you touched (then `cargo test --workspace --no-fail-fast` once at the end);
  `git diff --check`. For `app/` changes: `npm.cmd --prefix app run check`, `run test`, `run build`.
  Missing tooling is UNVERIFIED, never PASS.
- Hosted CI also runs clippy -D warnings on ubuntu. Anything used only under `#[cfg(windows)]` (imports, `mut`,
  helpers, test spellings with a literal backslash) must itself be gated or platform-neutral, or Linux fails
  with unused-import or unused-mut. Prefer `if cfg!(windows) { .. }` and `std::path::MAIN_SEPARATOR`.
- Sol lanes leave changes uncommitted: do not commit, push or open a PR; the driver reviews and commits.
  Never merge or edit coordinator-owned files: `AGENTS.md`, `.agent-harness/`, `HUMAN_TODO.md`,
  `docs/16-implementation-status.md` or `handoff/CHECKPOINT.json`.
- Final report (your last message, under 300 words): "left uncommitted", commands run with pass/fail counts,
  changed / verified / NOT verified / residual risk, and anything you noticed but did not do.
- STEP 0 (mandatory): before coding, list every item of your issue and verify OPEN vs FIXED on origin/main (merged PRs often said
  `Refs #N` and left the issue open). Cite file:line or the PR. Work only OPEN items; if none, report the table and stop.
