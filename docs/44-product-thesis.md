# Product thesis: workspace memory, honest by construction, agent-ready

Quadrant: explanation. Decided by the owner on 2026-10-12 (HUMAN_TODO q-25). This page says what
Loomward is for and how we would find out it is wrong. It is a statement of intent, not a status
report: [docs/16](16-implementation-status.md) records what exists, and the claims below are
only as strong as the evidence cited beside them.

## The thesis

Loomward is a model of the owner's work: what their files mean (this is a project, this is
finance, this belongs to a collection), kept separate from where the files live and from what may
touch them. Meaning, residency and permission are three systems, and none answers another. Every
claim Loomward makes carries its evidence and its blanks; unknown is never shown as zero. Agents
are another view over the same model, not a separate product.

Disk size, duplicates, backups, processes and tiers are views of the model. They are not the pitch.

## The three layers

| Layer | Claim | Backed today | Not yet backed |
|---|---|---|---|
| **Identity** | A model of the owner's work, separate from residency and permission ([README, three threads](../README.md#the-three-threads)) | Collections list files without moving them; a small student gives meaning advice that is shown in the Atlas and Explorer and is never an approval ([meaning threads](../evidence/v3/meaning-threads.md); personal learning on the owner's own labels, [docs/16](16-implementation-status.md), wave 8) | Accuracy and calibration on real data; the evidence above is synthetic |
| **Trust** | Each claim shows its evidence and its blanks | Unknown, partial and offline stay visible (product invariants in [AGENTS.md](../AGENTS.md)); telemetry reports `null` with a reason, never zero; a green backup job is never shown as verified recovery (ADR-V3-23, [docs/42](42-v03-adrs.md)) | Whether showing evidence improves owners' decisions (hypothesis H5) |
| **Growth** | Agents and MCP see the same model, scoped | A read-only MCP reference over a snapshot ([docs/28](28-mcp-specification.md), [UC-22](33-use-case-atlas.md)) | A live provider, authenticated agent identity, any effect |

## What each pillar is, relative to the thesis

| Pillar | Role | State (see [docs/16](16-implementation-status.md)) |
|---|---|---|
| Disk size (inventory, Atlas) | A view: what the model says about space | Native scans of granted folders; no whole-volume scan unless the owner picks one |
| Duplicates | A view; byte equality kept apart from similarity | Read-only content reads on owner grants; nothing is removed |
| Backups | A view of recovery evidence, not a verdict | Importing restic receipts; restore drills not built |
| Processes and RAM | A view of resource pressure | Read-only Observatory and telemetry |
| Tiers | A simulation of residency | A simulator; nothing is moved |
| Find & Ask | A view: fast search, plus an optional chat model that may call read-only tools (owner decision q-34) | Planned; nothing built, and I found no design document for it in the repository |
| MCP | The growth path | Read-only reference, as above |

No file or process effect exists, and this thesis does not change that. Effects stay behind their
own safety gates ([AGENTS.md](../AGENTS.md) invariant 1).

## The first five minutes

The honest version, with nothing assumed to be learned yet:

1. The owner grants one folder. Loomward lists its metadata and draws it by structure: folders,
   sizes, residency, and what it could not read.
2. Before any teaching, the student has an empty training set and abstains
   ([docs/00](00-design-brief.md), assumptions); the Atlas shows abstention as undyed, not as a
   guess ([meaning threads](../evidence/v3/meaning-threads.md)). Grouping at this point can come only
   from structure and markers such as repository roots and manifests. That is designed
   ([docs/24](24-thesis-and-research-program.md) section 3, [LW-010](../backlog/issues/LW-010.md)); I found no
   implementation of it, so treat it as not built.
3. The owner corrects a few items. The suggestions change across the folder and no path does.

How many corrections it takes before step 3 feels useful is unknown. That is the first thing to
measure.

## What would prove or falsify it

The hypotheses are in [docs/24](24-thesis-and-research-program.md) section 8; the evaluation
phases (fixtures, replay, shadow, small live study) are in section 9. None has been run on
personal data beyond mechanics.

| Claim | Hypothesis | Evidence that would count |
|---|---|---|
| Identity: virtual views beat folders alone | H1 | Shorter retrieval time with no increase in unwanted relocations |
| Identity: a student with selective help is affordable | H2 | Comparable accepted-suggestion quality with fewer corrections and teacher requests |
| Trust: evidence beats a bare score | H5 | More correct review choices when provenance and unknowns are shown |
| Growth: scoped views limit exposure | H6 | Less metadata sent to an agent without making answers wrong |

H3 and H4 (group-aware planning, shared resource evidence) test the placement and resource views,
not the thesis.

It fails if owner corrections cost more than the tidying they replace, or if owners prefer a few
explicit profiles to inferred behaviour (docs/24 section 11). The two halves fail differently. If
learning fails, the trust layer and the observation workbench still stand ([docs/24](24-thesis-and-research-program.md)
section 11), but the identity claim does not, and the product would have to be re-described.

## Audience path

The owner first, then power users, then mainstream Windows users, then developers and agent
users, then IT and small business (owner decision q-24). Linux comes later (q-37); the
three-systems ruling is platform neutral, and likely only the Windows provider crate
(`crates/loomward-windows`) would need a counterpart (inferred, not checked). This order is a
plan. The repository holds no user or market evidence.

## Alternatives rejected

- **The honest ledger alone** (every claim with its evidence, nothing more). Honesty is a
  property, not a reason to open the tool, and a read-only ledger can look like it does less. Kept
  as the trust layer.
- **A permission layer alone** (a broker for what people and agents may do). No effect exists yet,
  so it would govern nothing, and its audience today is developers. Kept as the growth path.
- **Make room alone** (a plan for what to free or move). Its best moment is a plan nobody can
  execute until an effect gate opens, and cleaners already promise a number and a button. Kept as
  views and simulation.

Sources for the comparison: the options draft written for the owner on 2026-10-12 (not committed).
