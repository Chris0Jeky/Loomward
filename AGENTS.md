# Working on Loomward (Codex adapter)

`CLAUDE.md` is the canon for every agent runtime: what Loomward is, how to run and prove each
seam, the product invariants, the pitfalls and the authority (T1, `.agent-harness/tier.json`,
`HUMAN_TODO.md`). Read it first; this file only carries what Codex needs beyond it.

- Shared Codex guidance, skills and roles come from sibling `claude-config/codex/` via
  `agent-harness/harness.py sync-global`. Do not copy global skills or MCP servers into this repo.
- As a peer worker you run in your own worktree on a disjoint slice. You cannot commit inside a
  worktree; leave the change staged-ready and report the proving command and its output. Never
  push, merge, or edit `CLAUDE.md`, this file, `.agent-harness/` or `HUMAN_TODO.md`.
- Use `codex.cmd`, `npm.cmd` and `npx.cmd` on Windows; the unsigned `.ps1` shims are blocked by
  this machine's execution policy.
- The ChatGPT-era `handoff/` files are product history. Their process rules (no push, no remote
  issues, inline review) do not apply here.
