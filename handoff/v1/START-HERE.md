# Start here: resume Loomward without losing the design

The source checkout is the product handoff. Do not regenerate it from the README or replace tested code with an empty scaffold. The portable reference already demonstrates observation, human-labelled learning, duplicate inspection, UI/API integration and tier simulation. Native integration is not yet demonstrated.

## First local session

Read `AGENTS.md`, `handoff/CHECKPOINT.json`, `docs/16-implementation-status.md` and `docs/00-design-brief.md`. Inventory the local tools with `python scripts/run.py doctor`. Use a new development branch. Start the demo, then run the reference suite. Preserve the original test outputs and add new platform-specific evidence; do not overwrite history and imply that the authoring pass ran on Windows.

Use `handoff/LOCAL-AGENT-PROMPT.md` as the first prompt for a local coding agent. The first work pack is **LW-001, LW-002 and LW-064**, respecting their dependencies. Establish the real Windows reference behavior, compile the Rust code and create a disposable filesystem fixture lab before building privileged or destructive capabilities. Work remains observation-only.

## Minimum launch

```powershell
py -3 scripts/run.py demo --open
py -3 scripts/verify.py
```

Python 3.11+ is the only prerequisite for the basic source demo. No LLM, Rust, Node, GPU, virtual environment or package installation is required to launch it. Node is used by the optional syntax-verification gate, Rust by native development and psutil by optional process observation. The browser test tooling is optional and not installed automatically.

`preview.html` is a self-contained synthetic preview. It is useful for reviewing information architecture but does not replace the running reference application. The server's token is in its printed URL fragment. Keep it private and do not remove it on navigation or refresh.

## Do not lose these design decisions

1. Semantic organisation and physical placement are separate. Virtual labels do not move files.
2. A model proposes; a deterministic, consent-bound native broker would execute. That broker is not implemented.
3. Protect project groups and path contracts before moving their contents. Application-owned state is not an ordinary document folder.
4. Unknown use is not disuse. Time since modification is not enough to permit retirement.
5. Copies, source allocations, target allocation requirements and eventual space relief are different quantities.
6. Persistent personal feedback and weak teacher labels keep separate provenance. A label is not an action grant.
7. Recoverability must be tested, not inferred from a successful copy, journal entry or backup exit code.
8. Process control starts with Loomward-owned workers and explicit cooperative providers, not global RAM trimming.

## Where decisions and evidence live

The numbered `docs/` files hold the product, Windows research, subsystem specs, security model, roadmap and experiments. `backlog/INDEX.md` provides the order and stable local IDs. `backlog/issues.json` is the machine-readable task contract with dependencies and body paths. `docs/21-requirements-traceability.md` maps the original request to present and future deliverables. `docs/original-request.md` preserves the user's brief. `evidence/` records passing checks, red test stages and limitations.

Use `docs/19-user-decisions.md` only for genuinely missing machine-specific choices. Do not ask the owner to restate the project, language preference or local-first intent. Do not stall on unknown drive letters: use disposable fixtures and keep live-disk adapters disabled until the owner selects scopes.

## Publishing and persistence

The provided source archive can be initialised as Git, or the accompanying local Git bundle can be cloned to preserve its initial commit. They do not establish a remote GitHub repository. Review tracked contents before publishing. The supplied publishing helpers are dry-run by default and require authenticated `gh`, an explicit owner and a clean committed checkout. Public visibility is a separate explicit decision.

At the end of each local work session, commit bounded changes locally and update the checkpoint with exact commit, test command, result, platform, remaining risk and next local issue. Do not enable auto-merge, push to an unrelated repository or import the complete backlog merely because a script exists.
