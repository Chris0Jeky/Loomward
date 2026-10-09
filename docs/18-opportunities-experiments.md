# Expansion map and experiments

This document preserves the ambitious scope without treating it as implemented. Every expansion should pay for its privileges, performance cost and maintenance burden with a measurable user benefit.

## Higher-value extensions
**Project/working-set intelligence.** Build groups from explicit project roots, manifests and user-confirmed relationships. Learn co-access only from consented event sources. Ask once whether a group belongs together, then keep its internal structure intact. Test whether grouping reduces corrections compared with file-by-file classification.

**A reclaimability ledger.** Explain where disk growth came from, which bytes are reproducible, which are retained for recovery, which are shared through hard links, which belong to backups and which remain unattributed. This is more useful than summing every “large” file. A future cleanup proposal gives a reclaimability range and verification conditions, not a guessed exact number.

**A storage digital twin.** Simulate capacity, copy scheduling, device disappearance, backup staging and access latency before running actions. Compare valid plans under different objectives. Use the simulator as a test harness for a future learned preference ranker, not as evidence that real file transfers succeeded.

**Semantic and near-duplicate search.** Exact duplicate detection, perceptual similarity and document revision similarity are different tasks. Similar images may be different crops the user needs; semantically similar documents may have different legal/project roles. Surface relationships first. Never route perceptual similarity into automatic deletion.

**A context-aware desktop.** Show a focused virtual view for a project or session, recent relevant items and unresolved decisions. Preserve the familiar desktop/Explorer fallback. Evaluate retrieval time and interruption burden; do not assume an animated/chatty companion improves usability.

**Growth and anomaly attribution.** Associate storage/commit growth with observed workloads and provider metadata. Ask “this cache has grown faster than its recent baseline” rather than “this is malware” or “this app leaks.” Forecasts need coverage and uncertainty. Avoid inventing application ownership from a path that merely looks familiar.

**Local agent integration.** Allow existing personal agents to request read-only summaries or propose a typed draft. The owner still approves effects through the same boundary. A shared action queue prevents multiple agents from racing to organise the same items. This can complement a larger software estate without making this app dependent on it.

**Tier-aware model management.** Track model bundles and which server sessions use them. Archive unused versions only through a provider that understands mappings/configuration. Stage an explicitly pinned model onto faster storage before a session. Coordinate memory release with the server rather than killing it. Measure whether model staging saves real wait time.

**Backup-aware organisation.** Keep files findable across live storage and retained snapshots through logical references, but avoid mounting everything or hydrating cloud data automatically. A “where can this be restored from?” answer should identify an actual snapshot and its tested fidelity.

**Explainable policy editor.** Translate a user statement into a visible constrained preference, show examples/counterexamples and request confirmation. Store policy version, scope and expiration. The LLM may draft a policy; it cannot self-install one or lower a security boundary.

## Experiments with decision rules
| Experiment | Baseline | Test | Keep it only if |
|---|---|---|---|
| Personal collection model | Extension/name rules and current student | Group/time-disjoint human-labelled holdout | Fewer corrections at equal abstention/interruptions |
| LLM weak labels | Human-only model | Add teacher events at low weight; inspect disagreements | Improves held-out utility without washing out human corrections |
| Embedding representation | Token student | Same labels, same splits, measured footprint | Material gain justifies model/embedding storage and latency |
| Heat predictor | Explicit pins plus simple decay | Offline replay with missing-coverage simulation | Better access latency/copy cost without excess churn |
| Greedy placement | Current solver | Small exhaustive feasibility/cost oracle | Gaps are understood, or a better solver is measurably justified |
| Interactive alternatives | Single suggestion | Two/three valid options with no-change | Higher understanding and lower unwanted approvals |
| Background governor | Fixed concurrency | Foreground latency under controlled stress | Less interference without starving useful maintenance |
| Virtual desktop | Ordinary folder/search flow | Timed retrieval and task-switching sessions | Faster work without hiding items or increasing mistakes |
| Incremental index | Bounded repeated scan | Restart/gap/rename storm corpus | Freshness and footprint improve without false completeness |

## Research questions worth retaining
How quickly can useful preferences be learned from a few dozen corrections? Can a model recognise project boundaries from metadata without content access? How should uncertainty propagate from incomplete usage observation into tier placement? Can the planner distinguish a temporarily quiet working set from truly cold material? What explanation helps a user spot a wrong group membership before approving a move? Which forms of undo actually increase confidence after the user edits files later? How much state can remain virtual before external application path expectations become the limiting factor?

## Defer unless justified
Cross-device cloud accounts, a custom filesystem, a kernel minifilter, global desktop replacement, biometric or location-based context, always-on screen capture, automatic app uninstall, registry cleaning, automatic destructive reinforcement learning and a proprietary backup format. These add substantial authority or complexity without being necessary for the first useful product.
