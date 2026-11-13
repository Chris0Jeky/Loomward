# Lane tooling

Use Git Bash on Windows, Python (`py -3`), Git >= 2.49, PowerShell and authenticated `gh`.
The estate's `~/.claude/tools/codex_safe.sh` and `estate_merge.py` remain machine-local prerequisites.

- `mkwt.sh <lane> <branch>` creates from `origin/main`, refusing an existing branch.
- `lane.sh <lane>` combines `brief.md` and a local prompt, runs Sol high, logs EXIT and completion.
- `queue.sh <lane>...` gates launches on RAM, disk and concurrency; run one queue process.
- `wait.sh <lane>...` wakes when **any** named lane finishes and prints all statuses.
- `review.sh <PR> <worktree> [BASE]` runs Terra high read-only on `BASE...HEAD` (`origin/main` by default).
- `detach.ps1 "<bash command>"` launches outside the session job object through WMI.
- `merge_queue.py [--once] [--dry-run]` considers only the first `<PR> <reviewed-sha> [<lane>]` line.
  It proves clean main merges on the first-parent path, current main, green CI and
  180 seconds of its own head observations before calling the estate merge helper.
  The optional lane contains only letters, digits, `_` and `-`; an invalid lane parks the line.
  After a successful merge, a named lane is removed by `bash <repo>/scripts/lanes/rmwt.sh <lane>`,
  with `LW_HOME` passed through and the queue's own working directory. Start the queue outside
  the worktrees it will remove. Two-field lines keep the existing behavior without teardown.
  Teardown logs its exit code and last output line; failure is logged once and the queue continues
  without retrying teardown. Copy anything needed out before enqueueing a lane for removal.
  Polls every 240 seconds; dry-run records observed heads but leaves the queue alone, makes no remote
  changes and calls the merge helper only with `--dry-run`. It logs the teardown command without running it.
- `rmwt.sh <lane>` lists ignored files, removes owned build/state directories and
  uses plain `git worktree remove`. Copy anything needed out first; run elsewhere.

Environment: `LW_HOME` defaults to `C:/lw`; `LW_REPO_SLUG` to `Chris0Jeky/Loomward`.
`MAX=3` and `MINMB=3000` control queue limits (disk floor: 25 GB).
`P` overrides the direct lane launcher's prompt stem; queue uses the lane name.
`CARGO_BUILD_JOBS` defaults to 3. Incremental builds are off, debug is line tables,
and each worktree uses its own default `target/` (the launcher unsets `CARGO_TARGET_DIR`).
Repository location comes from the installed scripts.
Example (driver commits and publishes between wait and review):
```sh
export LW_HOME=C:/lw
bash scripts/lanes/mkwt.sh example tooling/example
# Write $LW_HOME/prompts/example.md, then:
bash scripts/lanes/queue.sh example
bash scripts/lanes/wait.sh example
bash scripts/lanes/review.sh 123 "$LW_HOME/wt/example"
printf '123 %s example\n' "$(git -C "$LW_HOME/wt/example" rev-parse HEAD)" >> "$LW_HOME/mergeq.txt"
# Run from the coordinator checkout, outside the example worktree:
py -3 scripts/lanes/merge_queue.py
# A successful merge automatically attempts to remove example.
```
Smoke-tested 2026-11-13 (KRASPYON): `lane.sh` with a fresh `mkwt.sh` worktree ran `cargo test -p loomward-core`
under the wrapper's workspace-write sandbox, 30 passed, in the worktree's own `target/`. The sandbox has no network:
cargo needs a warm `~/.cargo` registry, so run `cargo fetch` in the worktree first when dependencies change.
Machine-local under `LW_HOME`: `logs/`, `prompts/`, soak runs, `wt/`,
`mergeq.txt`, `mergeq.parked`, `mergeq.seen.json` and `logs/queue.lanes`.
