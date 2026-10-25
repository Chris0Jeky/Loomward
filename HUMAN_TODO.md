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

## q-5 — Teacher on real metadata: disclosure scope and tier — ANSWERED 2026-10-11 (scoped; waits on the broker's canaries)

The owner chose GPT-6.1 Sol (medium) through the local Codex CLI as the learning teacher, a connected
cloud provider and so a tier re-review trigger. The confinement spike (#116) showed the CLI cannot be
fully confined by flags (one collaboration tool stayed callable). **Owner, in chat 2026-10-10:
authorise the OS-enforced sandbox work (LW-111, lane L15b).** Until its enforcement canaries pass,
the teacher sees synthetic metadata only. Originally asked (answered below on 2026-10-11, when the repo also moved to T2): may real
names and paths leave the machine, in what scope, and does the tier stay T1 or gain `sensitive_data`?

**Owner, in chat 2026-10-11 (interactive choice), selected: "Scoped: names only, per-request preview"** — after the broker passes
its enforcement canaries: file and folder names only (no full paths, no contents), shown in a preview for every request,
and the repo gains the `sensitive_data` overlay at that point. Until the canaries pass the teacher stays synthetic-only.

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
  security change; q-5 personal-data disclosure was answered on 2026-10-11 (names only, after the canaries).
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
- q-5 personal-data disclosure was answered on 2026-10-11 (names only, per-request preview, after the broker's
  canaries; the repo is T2 and gains sensitive_data then). No claim of
  OS-enforced endpoint isolation follows from validating requests in application code.
- Read-only diagnosis of the separate Kraspyon CLI/model and sandbox startup problems was also
  approved. Machine access must be available and authorised before diagnosis; this does not
  permit changing model entitlements, authentication, ACLs, or runtime security settings.
- The linked `decisions/loomward-teacher-sandbox-path.json` was not present in this repository
  when checked. This section is the durable repository record; no external decision inbox
  has been marked complete by this change.

## q-9 — Revoked personal roots: keep the names until you forget them, or purge on revoke — ANSWERED 2026-10-11 (a)

Personal scans are open. Revoking a personal root stops access but keeps its display path and catalogue rows in
Loomward's local state (Loomward#288). Nothing leaves the machine. Posed through the agent-hq inbox on 2026-10-11;
the expiry default (option (a) on 2026-10-18) no longer applies: the owner answered (a) in chat on 2026-10-11 (below).

- [x] **loomward-revoked-root-retention: When you revoke a personal scan root in Loomward, should its name and catalogue stay on your machine until you choose to forget it, or be purged automatically?**
  - (a) Keep the audit trail; add a 'Forget this root' action (recommended): Revoke keeps today's behaviour. Agents add an explicit, user-initiated 'Forget this revoked root' command and button that purges the display path and catalogue rows and keeps an anonymised audit entry (that a root was granted, revoked and forgotten, with dates, no name).
    + You keep a record of what you revoked until you decide otherwise; + Forgetting is a deliberate act you can see; - Names of revoked roots stay on disk until you press Forget
  - (b) Purge automatically on revoke: Revoking a personal root also purges its display path and catalogue rows at once, keeping only the anonymised audit entry.
    + Nothing named survives a revoke; - No way to see later which folder you had revoked; - A mistaken revoke loses the catalogue (a re-scan rebuilds it)
  - (c) Keep as is: No change: revoked roots stay listed by name in the audit view, and their catalogue rows stay in state.db.
    + No work; - The storage-limitation finding stays open
  - Why: It keeps a visible audit trail by default and gives you a deliberate, explicit way to remove names, without surprising data loss on a mistaken revoke.
  - Why an agent may not decide: It sets how long Loomward keeps the names of your personal folders after you revoke them: a personal-data retention choice that is yours to make.
  - Source: agent-hq `decisions/loomward-revoked-root-retention.json` (the owner inbox; not a file in this repository)
  - **Owner, in chat 2026-10-11, selected (a): "Keep, plus a 'Forget' button".** Implementation (the Forget command and button) is tracked as
    Loomward#288; this item records the decision, not the implementation.
  - Implemented: the Forget command and button landed in #382 (2026-10-11), closing #288. Residual items are #390 and #391.

## Owner decisions of 2026-10-11 (interactive choices in chat)

Recorded from the owner's selections; each option's text was shown with its trade-offs. Implementation items stay open
until their PR lands; an agent checks one off only when the change is merged and proven.

### q-10 — Duplicates on personal roots: content reads — ANSWERED: open, local read-only hashing

Selected: "Open: local, read-only hashing". Staged size, partial hash, full hash and byte comparison through pinned,
revalidated, non-elevated handles with no-recall opens; cloud placeholders skipped; nothing leaves the machine; nothing is
deleted; bounded by the job's byte and time budgets. Retires `content_read_gated` for owner-granted personal roots.
- [x] Implemented and merged: #376 (docs/16 and CHECKPOINT updated there; AGENTS.md invariants carry no q-10 gate wording, so none changed).

### q-11 — Learning on personal roots — ANSWERED: open locally, the owner's labels only

Selected: "Open locally, your labels only". The local student trains on the owner's labels plus metadata features of
granted roots, on this machine; suggestions stay advice with relative scores; the teacher stays synthetic until q-5's
conditions hold. Retires `learning_personal_gated`.
- [x] Implemented and merged: #368, with #372's filesystem record. #368's 9fa5159 labels a file in a real granted NTFS folder.

### q-12 — FAT and exFAT volumes (#219) — ANSWERED: list them, marked identity-unverified

Selected: "List them, marked unverified". Metadata only, identity quality `unverified`; such rows never back a proposal or
effect; relists recheck containment by path; a personal root on such a volume is confirmed explicitly at grant time.
- [x] Implemented and merged: #372 (real FAT volume leg unverified).

### q-13 — Catalogue write speed (#148) — ANSWERED: both retarget and push

Owner, verbatim: "a mix of the first two options": the KPI becomes end-to-end files/s on lab and real trees (250k rows/s
stays a stretch note), and an Opus high lane designs a first-scan bulk-load path (fresh table, deferred indexes, atomic swap
under the publication fence), A/B measured on a quiet host.
- [x] KPI retargeted in evidence; bulk-load path landed with evidence: #387 (quiet-host A/B still unverified, #148).

### q-14 — Backups next step (LW-084) — ANSWERED: the driver chooses the best-engineered path

Owner, verbatim: "I'm not sure about the best way forward about this, I'm happy with the best engineered solution you can
think of, even if it requires additional scope and work". The driver's plan is recorded in docs when the lane starts.

### q-15 — Diagnostic bundles (LW-055) — ANSWERED: native save dialog

Selected: "Native save dialog (desktop app)". The desktop app writes the redacted bundle to a file the owner picks in a
Rust-side save dialog; this is Loomward's first file write and lands with its own small safety gate (path never crosses
IPC; only the previewed bytes; new file or owner-confirmed overwrite).
- [x] Implemented and merged: #399 (2026-10-12; ADR-V3-24; Loomward asks its own Cancel-default question before replacing a
  file). Not yet observed by a person: the real save dialog and the replace box (#437, q-45).

### q-16 — Tier and posture — ANSWERED: T2, with lower weight on privacy and security

Owner, verbatim: "T2 but from now forward give less importance to the "privacy" and "security" aspect of all of this ,
I don't have high sensitivy info or data, so the risks aren't as great". Declared T2 in `.agent-harness/tier.json`.
Driver's reading, reversible by the owner: privacy and security findings weigh lower and get no extra lenses; hardware is
no longer withheld from new evidence; secrets in Git and real file names, paths or inventories in this public repository stay
forbidden because they are irreversible.

### q-17 — Releases (LW-057) — ANSWERED: unsigned preview pre-release

Selected: "Unsigned preview pre-release": a GitHub pre-release of the Tauri desktop app, unsigned, marked preview, after T2.
- [x] Release workflow and first preview pre-release published: v0.3.0-preview.1 (2026-10-12).

### q-18 — Older hardware mentions (#283) — ANSWERED: scrub to generic labels

Selected: "Scrub to generic labels". Media and core counts become generic labels; the mock volume capacities become
generic round numbers. Git history keeps the old text.
- [x] Scrub merged: #365 (docs and mocks), #394 (raw receipts and old Python fixtures) and #396 (one UX example).
  Not inspected: the PNG screenshots under `evidence/v3/`.

### q-19 — Dependabot alert #182 (glib, Linux-only) — OWNER ACTION: dismiss as not reachable

Selected: "Dismiss as not reachable". Agents never change security alerts. Steps:
1. Open https://github.com/Chris0Jeky/Loomward/security/dependabot/1
2. Click "Dismiss alert", choose "Vulnerable code is not actually used", and add the comment: "Linux GTK path; Loomward
   ships Windows only (Tauri on WebView2). Re-check on each Tauri upgrade."
- [ ] Dismissed (verify: `gh api repos/Chris0Jeky/Loomward/dependabot/alerts/1 --jq .state` prints `dismissed`).

## Owner decisions of 2026-10-12 (guided walkthrough in chat)

Interactive rounds with context, options and a recommendation each (decision map `map:v1:48e85b120e305470cd8c1778231a34b73efb2628b284fd70b8565a7e08c71970`
for round 1). The owner selected the recommended option in each.

### q-20 — Licensing route — ANSWERED: copyleft plus paid exceptions

Selected: "Copyleft + paid exceptions". Open-source copyleft for everyone; the owner keeps the right to sell proprietary
licences (dual licensing). Owner's framing: "I don't want MIT for this project, I don't just want it to be used by anyone for
free, I want it to be at least gplv3 or something similar". Code published before the relicensing commit stays MIT for anyone
who received it.
- [x] Relicensing merged: Loomward#396 (2026-10-12).

### q-21 — Licence text — ANSWERED: AGPL-3.0-only

Selected: "AGPL-3.0-only": GPLv3 plus the network clause; moves to later versions only by the owner's choice.

### q-22 — Outside contributions — ANSWERED: a CLA checked by a bot

Selected: "CLA via a PR bot". `CLA.md` (adapted Apache ICLA, with an explicit relicensing right) lands in #396. The Actions-based
CLA bot is archived, so the hosted CLA Assistant is used; connecting it is an OAuth grant, the owner's action. Steps:
1. Sign in at https://cla-assistant.io with GitHub as @Chris0Jeky and authorise the app.
2. Choose "Configure CLA" and pick `Chris0Jeky/Loomward`.
3. As the CLA source, paste the text of `CLA.md` into a new GitHub gist and select that gist.
4. Exclude `Chris0Jeky` and bots (`*[bot]`, for example `dependabot[bot]`).
5. Ask for name and GitHub login only.
6. After the bot first reports on a PR, an agent can add `license/cla` as a required check on `main` when you say so.
- [ ] Owner read `CLA.md` before the first outside contribution.
- [ ] CLA Assistant connected (verify: the `license/cla` status appears on the next PR).

### q-23 — The name — ANSWERED: clear it and add a fork policy

Selected: "Clear it + fork policy". `TRADEMARKS.md` (modified forks must rename) lands in #396. Clearance search of 2026-10-12:
AMBER, nothing blocking found. `loomward.com` is held by an unrelated shop. A GitHub organisation `Loomward` exists, created
2026-08-06, empty, owner unknown. `.app`, `.dev`, `.org` and `.io` appeared unregistered. crates.io, npm, PyPI, Chocolatey and
Scoop Main are free. The trademark registers (USPTO, EUIPO, UKIPO, WIPO) could not be searched by an agent (JavaScript-only
search pages); "Loom" (Atlassian) is the nearest mark, judged low-to-moderate risk.
- [ ] Owner hand-search of the four registers for LOOMWARD in classes 9 and 42 (about 15 minutes).

## Owner decisions of 2026-10-12, rounds 2 to 7 (guided walkthrough in chat)

Each question came with context, options and a recommendation. Final decision map, rounds 1 to 7:
`map:v1:32675268460511cc436407354aec933d652538e7cc8ce0fc2b67c59f75d2387a`. Quotes are the owner's words.

### q-24 — Audience — ANSWERED: the owner first, then power users, then wider

Selected "You first, then power users". Later: mainstream Windows users, developers and agent users, and IT and
small-business admins ("since development is going well and it's very quick"). Possibly Linux users too (q-37).

### q-25 — Thesis — ANSWERED: workspace memory, honest by construction, agent-ready

First asked as the preview's hero feature. The owner deferred: "I'm not sure the "hero" feature is just the disk
explorer.... it's so much more ... I'd like to find the best thesis for this yet (you can reason on and do this for
me)". Shown four candidate theses, the owner selected "Memory + ledger + agents":
- **Identity:** a model of the owner's work, kept separate from where files live and what may touch them.
- **Trust mechanism:** evidence and explicit blanks.
- **Growth path:** agents and MCP.
- [x] Thesis doc and README positioning merged: #403 (docs/44).

### q-26 — Money — ANSWERED: not now; keep the option

### q-27 — Success in six months — ANSWERED: daily use, a public preview others use, a research write-up

"I'm actually planning on using it daily as soon as it becomes stable, extremely well tested in synthetic but extreme
scenarios to prove its safety, and as soon as all the features and the UX are done genuinely".

### q-28 — Code signing — ANSWERED: unsigned for now; the Store and a certificate soon

"Unsigned for now but I'll go with the Store and signing certificate in the near future so agents should account for
that". Agents keep the release pipeline ready for a certificate and a Store submission.
- [ ] Owner opens a Microsoft Store individual developer account (free; identity check).
- [ ] Owner buys a code-signing certificate.

### q-29 — Channels — ANSWERED: GitHub Releases, winget, a Scoop bucket and the Microsoft Store

Publishing to winget, Scoop or the Store for the first time is still a tier re-review trigger under AGENTS.md invariant 8;
this answer chooses the channels, it does not pre-authorise those publications.
Prepared: #425 (Scoop placeholder bucket, winget generator, Store and signing runbooks); nothing published.

### q-30 — Updates — ANSWERED: an opt-in update check

Off by default: a "Check for updates" button and an optional check-on-start toggle. Invariant 2 is unchanged.
- [ ] Owner generates the updater signing key and stores it as a repository secret (steps arrive with the updater PR).

### q-31 — First preview — ANSWERED: right after the AGPL change, then one per wave

Also: "for my personal use (which will be done on this machine as well as on Kraspyon , the second machine) I don't
want to wait for releases for the new features etc, the upgrades should be seamless and streamlined directly into the
working state" (q-33).

### q-32 — First real effect — ANSWERED: copy-and-verify

It never deletes the source and lands behind its own safety gate. AGENTS.md invariant 1 changes only with that gate.
Design merged: #423 (ADR-V3-25, docs/47). The gate is not open. Owner questions Q1 to Q3 are in docs/47 section 12
and are asked when lane C2 starts (q-47).

### q-33 — The owner's machines — ANSWERED: a public nightly pre-release

The selected option: every merge to `main` publishes a rolling, unsigned nightly pre-release, and the opt-in updater
on the owner's machines follows it. Driver's plan, stated in chat: state is copied before every migration and the
previous nightly stays restorable. AGENTS.md invariant 8 names the nightly as owner-authorised.
Implemented: #432. The first nightly, 0.3.0-nightly.20261012.1, published on 2026-10-12. Update installs stay off until
the owner's updater key exists (q-30); rollback is manual (RELEASING.md).

### q-34 — New scope — ANSWERED: Find & Ask, plus four areas

"Based on the ability to organise and create structures (for files and folders): the ability to easily find what is
needed, potentially also having a smaller model with which one can chat and ask about the files, apps, etc and it can
find anything, list, etc , so both the search engine that is extremely well-optimised and engineered for efficiency and
robustness, as well as a way for a small model to be integrated seamlessly and to give it the ability to use it (though
it should be easily usable by humans too), and the model could also be a local one".

Also selected: developer clutter, content understanding, cloud and sync awareness, and monitoring and alerts. The model
gets read-only tools over the same query surface as people and MCP agents. Anything it suggests doing becomes a proposal
the owner approves.
- [x] LW items minted for Find & Ask and the four areas: #422 (LW-113 to LW-130).

### q-35 — Where the model runs — ANSWERED: one provider interface

The local OpenAI-compatible endpoint the owner runs comes first. Cloud with the owner's own key follows, under q-5's
disclosure rules. A bundled engine comes later. Downloads happen only on explicit consent (invariant 2).

### q-36 — Search reach — ANSWERED: maximum capability

"The most effective choices possible in terms of performance and capabilities. no matter the engineering cost."
- **The backbone:** Loomward's own full-text and substring index over granted roots, kept live by the non-elevated watchers.
- **An optional elevated NTFS-metadata helper** for instant whole-volume search, designed behind its own gate. The
  invariant 2 change lands only with that gate.
- **Everything integration** where it is installed.
- **Plus "whatever extra":** the driver adds a local semantic (embedding) layer.

Design merged: #426 (docs/46; the helper reads USN journal records only, never raw MFT records). Search v1 merged:
#438 (owner-local FTS5 index; MCP and Ask exposure waits on the LW-069 adapter gate, #429).

### q-37 — Linux — ANSWERED: Windows first, Linux later

An observe-only Linux port comes after the Windows daily-use bar. The core stays portable meanwhile.

### q-38 — Connectivity — ANSWERED: local only for now; the rest when there is capacity

"the ability to have have a NAS would be nice, but all the focus for now should be for local only, the rest is optional
and nice to haves if there is capacity". Nice-to-haves: NAS shares as roots, paired machines, remote MCP, cloud sync.

### q-39 — Daily-use readiness bar — ANSWERED: the proposed bar

The bar: crash injection at every write, the 10M-entry lab, pathological trees, churn during scans, injected I/O faults
and a full disk, fuzzing, and a 72-hour soak. Every effect also needs power-cut and proven-restore evidence. The owner
alone judges "UX done".
- [x] Readiness checklist merged: #427 (docs/45; crash injection at every catalogue write).
- [ ] Every line carries pass evidence.

### q-40 — Old code — ANSWERED: freeze the Python reference, retire the legacy browser UI

The Python reference stays as a frozen test oracle. The legacy browser UI and `preview.html` retire once parity with the
Svelte app is shown.
- [x] Implemented: #424 (parity table in docs/30).

### q-41 — Pace — ANSWERED: maximum throughput

More lanes, plus Muse swarms for test writing and bug hunts. The owner warned that usage limits will likely be reached
tonight and reset later in the day.

### q-42 — Domains — ANSWERED: GitHub only for now

"like for taskdeck, this will likely fall under the deliverasoft umbrella domain, but let's leave it for github only for
now".

### q-43 — Priorities under usage limits — ANSWERED: no preference; the driver's plan order stands

"I don't have a preference, I'm not gonna start using it tomorrow, things will advance as planned".

### q-44 — UX steering — ANSWERED: agents decide for now

Agents use impeccable-style prototypes and A/B variants for the owner to try and criticise. Per-area review sessions,
and possibly a full redesign, come later. "for now I wanna keep the momentum going".

### q-45 — The owner's hands-on items — OWNER ACTIONS (planned for the next few days; the q-19 and q-22 boxes are the canonical ones)

- [ ] Connect the CLA bot (q-22 steps).
- [ ] Dismiss the Dependabot alert (q-19 steps).
- [ ] Install the preview on this PC and Kraspyon and enable the nightly channel, once both ship.
- [ ] Hardware tests: a FAT or exFAT USB stick (q-12), and a folder granted through the real picker.
- [ ] Save a diagnostic bundle once to a new file and once over an existing file in the desktop app, to see the real save
  dialog and Loomward's replace box (#437).

### q-46 — Repository home — ANSWERED: stays under Chris0Jeky

An empty GitHub organisation named `Loomward` (created 2026-08-06) exists. Whose it is was not established.

### q-47 — Design questions waiting for their lanes — copy-and-verify Q1-Q3 and Ask Q3-Q4 DRAFTED 2026-10-17

Copy-and-verify lanes C1 to C5 and C7 landed on 2026-10-17 (lab-only), so Q1 to Q3 were drafted for the owner
inbox in agent-hq#461, and Ask Q3 (local-model preview) and Q4 (conversation retention) were added there the same day;
they reach the cockpit when that PR merges. Answer there or in chat. Still not asked: the elevated helper's launch model
(docs/46 Q1), Everything results outside the grants (docs/46 Q2) and the Store item below.
- [ ] **loomward-copy-staging-removal** (Q1): may cancel and abandon delete Loomward's own unpublished staging files,
  matched by recorded identity inside its own staging folder? Recommended: allow. Until answered, cancel and abandon
  leave the staging folder in place and report it (`LOOMWARD_COPY_STAGING_REMOVAL = false`, #522).
- [ ] **loomward-copy-power-cut-method** (Q2): a Hyper-V VM hard-stopped during lab copies (desktop only; KRASPYON is
  Windows 11 Home, without Hyper-V), a power pull on a disposable machine, or neither yet (copy stays lab-only).
  Recommended: Hyper-V. Gate G5 stays shut until one is done.
- [ ] **loomward-copy-destination-shape** (Q3): v1 new folder only (recommended, built), or also into an existing
  folder with new names only. Defaults to new folder only on 2026-10-24 if unanswered (reversible).
- [ ] **loomward-ask-local-preview** (docs/46 Q3, posed 2026-10-17 in agent-hq#461): for a model on this machine, is
  the per-folder grant enough (recommended, what the local provider #556 and the Ask panel #409 build), or also a preview
  before every request? Defaults to the grant only on 2026-10-24 if unanswered (reversible).
- [ ] **loomward-ask-retention** (docs/46 Q4, posed 2026-10-17 in agent-hq#461): keep Ask conversations until you
  delete them (recommended; Forget cascades) or delete them after N days. Defaults to keep on 2026-10-24 if unanswered.
- Find & Ask, not asked yet (docs/46 section 11 names each question's trigger lane):
  - the elevated helper's launch model;
  - Everything results outside the grants (hidden is recommended).
- Microsoft Store: a short privacy policy page is required before a Store submission (packaging/store/README.md).

### q-48 — External archive disk (H:) and the first cleanup — POSED 2026-10-25 (agent-hq inbox)

The owner attached a WD Elements USB disk (`H:`) for backup and archive and asked for safe archiving, agent tooling and
a supervised cleanup. Design: [docs/48](docs/48-external-disk-and-agent-storage.md). Copying personal files still waits
on the copy gate (q-47 Q1 and Q2 above); these two decide the cleanup around it:
- [ ] **loomward-first-cleanup-copier**: before Loomward's own copier passes its gate, (a) wait, (b) agents copy with
  robocopy per approved plan and Loomward's read-only verifier proves each copy (recommended), or (c) you copy in
  Explorer and the verifier proves it.
- [ ] **loomward-archive-originals**: after a verified copy, (a) you delete originals from Loomward's verified list,
  (b) Loomward sends them to the Recycle Bin after your per-plan confirmation, behind its own gate (recommended), or
  (c) permanent delete after confirmation.
