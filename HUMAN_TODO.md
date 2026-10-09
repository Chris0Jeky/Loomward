# HUMAN_TODO — Loomward

Only actions an agent cannot safely complete belong here: accounts, agreements, identity,
spending, real-machine permissions, or a material product choice. Agents check an item off only
when completion is directly verified; never infer a human decision.

## q-1 — Confirm the T1 sandbox tier — OPEN

Declared T1 (push and merge free) on 2026-10-09 with no production signals. Proposed promotion
to T2 at the first native file or process effect, real data, published release or connected
model provider (`.agent-harness/tier.json` notes). Confirm T1, or name a different tier.

## q-2 — Confirm Muse proxy-only worker network — OPEN

`.agent-harness/delegation.json` lets Muse workers reach package registries through the sandbox
proxy so Rust and Python proving checks can run delegated. Based on the founding-session direction
to use Muse lanes heavily. Confirm, or ask for `restricted` (delete the file).

## q-3 — Product direction — ANSWERED 2026-10-09, CLOSED

Answered by the owner in the founding session (chat, 2026-10-09). Push all four pillars, sequenced
by the driver: native engine (fast Windows scanner, identity, volumes, persistent catalogue) with
a new UI on top; organisation and learning; storage balancing (simulated against the real
volumes, nothing moved); resource companion (read-only telemetry). Also: optimisation, general
improvements, testing, validation, and a sandbox for stress-testing. More Sol and Muse lanes are
allowed for the volume of work. Visual identity: "Woven atlas" as the primary signature, plus an
"Observatory" mode (radial sunburst, live gauges) and more views for monitoring everything.

## q-4 — Real-disk scan root — ANSWERED 2026-10-09, CLOSED

Answered by the owner in the founding session: synthetic trees first for velocity, then agents
may pick a few big real folders themselves for read-only metadata stress tests (authorised).
Bounds that still apply: metadata only (names, sizes, times; never file contents), non-elevated,
state kept outside Git and outside the scanned root, no real file names in commits, issues or
fixtures. Never a volume root, a whole user profile, or a credential or browser-profile store.
Each agent-picked root is recorded in the gitignored `.loomward/` state before it is scanned, so
every real scan still has an explicit, recorded root; public evidence uses anonymised labels and
aggregate counts (AGENTS.md invariant 2 now carries this grant).

## q-5 — Teacher on real metadata: disclosure scope and tier — OPEN

The owner chose GPT-6.1 Sol (medium) through the local Codex CLI as the learning teacher. That is
a connected cloud model provider, which `.agent-harness/tier.json` lists as a tier re-review
trigger. Current assumption: the teacher sees **synthetic** metadata only; every request carries
an explicit, bounded disclosure grant. Before any real file metadata is sent to it, the owner
confirms (a) whether real names and paths may leave the machine, and in what scope, and (b) whether
the tier stays T1 or `sensitive_data` is set.
