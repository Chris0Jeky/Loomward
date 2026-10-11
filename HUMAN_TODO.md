# HUMAN_TODO — Loomward

Only actions an agent cannot safely complete belong here: accounts, agreements, identity,
spending, real-machine permissions, or a material product choice. Agents check an item off only
when completion is directly verified; never infer a human decision.

## q-1 — Confirm the T1 sandbox tier — CONFIRMED 2026-10-10, CLOSED

Declared T1 (push and merge free) on 2026-10-09 with no production signals. Proposed promotion
to T2 at the first native file or process effect, real data, published release or connected
model provider (`.agent-harness/tier.json` notes). Confirm T1, or name a different tier. **Owner, in chat 2026-10-10: confirmed T1.**

## q-2 — Confirm Muse proxy-only worker network — CONFIRMED 2026-10-10, CLOSED

`.agent-harness/delegation.json` lets Muse workers reach package registries through the sandbox
proxy so Rust and Python proving checks can run delegated. Based on the founding-session direction
to use Muse lanes heavily. Confirm, or ask for `restricted` (delete the file). **Owner, in chat 2026-10-10: confirmed.**

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

## q-5 — Teacher on real metadata: disclosure scope and tier — OPEN (sandbox authorised)

The owner chose GPT-6.1 Sol (medium) through the local Codex CLI as the learning teacher, a connected
cloud provider and so a tier re-review trigger. The confinement spike (#116) showed the CLI cannot be
fully confined by flags (one collaboration tool stayed callable). **Owner, in chat 2026-10-10:
authorise the OS-enforced sandbox work (LW-111, lane L15b).** Until its enforcement canaries pass,
the teacher sees synthetic metadata only. Still open for the owner, after the canaries pass: may real
names and paths leave the machine, in what scope, and does the tier stay T1 or gain `sensitive_data`?

## q-6 — One-time elevated firewall rule for the teacher sandbox — NOT YET NEEDED

LW-111 will likely need an outbound firewall rule restricting the sandboxed teacher to the model
endpoint. Changing system security settings is the owner's action, never an agent's. When the lane
reaches it, it prepares the exact command and a revert command here; the owner runs them.

## q-7 — Whole-volume scan roots and the lab size — ANSWERED 2026-10-10, CLOSED

Owner, in chat: whole volumes (`C:\`, `G:\`, `E:\`) may be personal scan roots when the owner picks them
in the native grant dialog (ADR-V3-08); agents still never pick a volume root. The 10M-entry synthetic
lab tier runs on `E:\loomward-lab\scale` only; `G:` stays capped at 2M (ADR-V3-17).

## q-8 — Teacher sandbox path — ANSWERED 2026-10-10 (b), SUPERSEDED 2026-10-10/11 BY (c) BOTH

**Superseding answer.** In a later chat (agent-hq decision `loomward-teacher-sandbox-path`, recorded in
agent-hq#356 at 22:46 UTC on 2026-10-10) the owner answered (c) "Both": build the broker, and allow
the one-off profile registration purely to measure what an AppContainer enforces, deleting it
afterwards. On 2026-10-11, shown that (b) and (c) disagreed, the owner chose, verbatim: "(c) and
allow re-measuring". Recorded scope now:

- The broker (b) remains the design. Agents build it with synthetic fixtures first.
- Agents may register the one named test profile `Loomward.L15b.Measure` again when a measurement
  needs it, and must always delete it afterwards and prove deletion (Loomward#291 did this once).
- Unchanged guards: never copy or reuse an existing Codex credential; a new sign-in for the broker is
  a separate owner approval; no firewall rule (q-6 stays the owner's); no other ACL or Windows
  security change; q-5 personal-data disclosure stays open.
- The "No AppContainer profile registration" line below is superseded by this paragraph for the
  named test profile only.


The L15b spike (#174) found that an AppContainer launch without a registered profile is refused by
Windows before anything runs, so no enforcement could be measured. Owner chose option (b), inference broker first, in chat on 2026-10-10 at 18:51 UTC.
Authorised now: bounded broker design/development and synthetic-only tests. No personal metadata
is sent until q-5 is separately answered. This is an architecture decision, not evidence that a
broker is already integrated or qualified.

- [x] **loomward-teacher-sandbox-path: How should Loomward's teacher (Sol via Codex) be confined before it may ever see your real file metadata: register a Windows AppContainer profile, or build an inference broker instead?**
  - (a) Register an AppContainer profile for the spike: Agents may call CreateAppContainerProfile for one named Loomward test profile under your account, rerun the L15b canaries (filesystem, network, tools, child processes) and delete the profile afterwards. The firewall rule (q-6) still waits for you if endpoint-only egress is wanted.
    + Measures real OS enforcement on this machine; + Removable with DeleteAppContainerProfile; - Writes a per-user Windows registration; - Contained Codex never started, so authentication remains untested; a broker may still be useful; - Endpoint-only egress still needs q-6
  - (b) Build an inference broker instead (recommended): Agents design and build a small broker: it accepts only bounded requests from Loomward and uses a supported inference-only transport with no file, shell or child-agent tools. Start with synthetic tests. Explicit app authentication, account/model eligibility, personal-data disclosure (q-5), and any paid API fallback are separate gates; do not copy or reuse an existing Codex credential. Endpoint selection in code is not proof of OS-enforced egress isolation. No Windows profile or firewall change.
    + No Windows registration or firewall change; + Avoids relying on a full agent executor; supported authentication still needs verification; + The boundary is code we test, not a policy we hope holds; - More engineering than option a; - Its own security review
  - (c) Both: broker as the design, profile only to measure: Build the broker (b), and also allow the one-off profile registration (a) purely to measure what an AppContainer enforces, deleting it afterwards.
    + Most evidence; - Most work; - Still writes the per-user registration
  - (d) Not now: keep the teacher synthetic-only: No further sandbox work; the teacher stays on synthetic data indefinitely and LW-111 stays blocked.
    + No new trust boundary; - The learning pillar never uses the teacher on your real organisation
  - Why: The spike observed a pre-execution launch refusal; it did not test authentication or confinement. A provisioned-profile retry is an experiment, not a proven fix. A broker reduces the capabilities that must be contained, provided its transport is genuinely inference-only and its authentication is explicitly approved.
  - Why an agent may not decide: It decides whether agents may write a Windows security registration under your account and which trust boundary guards your personal metadata; that is yours to choose.
  - Source: `decisions/loomward-teacher-sandbox-path.json`


### Recorded scope of the q-8 choice

- Build and test a small inference-only boundary with synthetic fixtures first. No shell,
  filesystem, MCP, child-agent, or arbitrary tool execution exposed by its request contract.
- App authentication and account/model eligibility remain unverified. Do not copy existing
  Codex credentials or silently fall back to a paid API. New authentication is a separate approval.
- No AppContainer profile registration, ACL edits, firewall changes, or other Windows security
  changes are authorised by this choice. q-6 remains separate.
- q-5 personal-data disclosure and provider-connected tier review remain open. No claim of
  OS-enforced endpoint isolation follows from validating requests in application code.
- Read-only diagnosis of the separate Kraspyon CLI/model and sandbox startup problems was also
  approved. Machine access must be available and authorised before diagnosis; this does not
  permit changing model entitlements, authentication, ACLs, or runtime security settings.
- The linked `decisions/loomward-teacher-sandbox-path.json` was not present in this repository
  when checked. This section is the durable repository record; no external decision inbox
  has been marked complete by this change.
