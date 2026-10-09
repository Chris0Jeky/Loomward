# Thesis: learned organisation without delegated control

Status: proposed research programme and product argument, not a peer-reviewed paper or a completed empirical study. The experiments in this package test reference algorithms on synthetic data. They do not establish user benefit, native Windows performance or model quality.

## 1. The problem is not simply untidy folders

A digital workspace has several simultaneous structures. There is a physical filesystem, a conceptual organisation, a set of applications with assumptions about paths, a history of use, a dependency structure and a resource budget. Traditional folder organisation often forces one tree to stand in for all of them. A client invoice may belong to a project and to an accounting period. A video project may contain source footage, proxies, caches and exports with very different recoverability. A large model may be cold most of the week and indispensable for a scheduled job.

A tool that only sorts by extension cannot express those distinctions. An unrestricted LLM can describe them but does not, by reasoning alone, obtain reliable file identity, recoverability or control rights. A more promising architecture lets learned preferences choose useful views and rank alternatives while independently verified constraints bound effects.

The research question is: **Can a local, selectively assisted learning system improve retrieval and resource decisions while reducing the amount of attention and authority demanded from its owner?** The answer remains empirical. The design makes it measurable rather than assuming that more automation means more value.

## 2. Three distinct variables

For object or dependency group g at time t, distinguish semantic membership S(g,t), physical residency R(g,t), and allowed operations A(g,t). Membership may be many-to-many. Residency may include an original, a cached replica and an independent backup. Authority is the intersection of an explicit grant, present preconditions and the execution system's supported operations.

A semantic classifier estimates a preference; it does not populate A. A placement optimiser proposes a change in R; it does not populate A. A successful tool response proves that a request completed, not that an owner approved a destructive action. The proposed architecture is valuable partly because it makes these invalid inferences difficult to express in code.

This separation also creates a low-risk learning path. An owner can teach “show these under Finance” before the system ever learns to propose a move. A correction to a virtual membership is substantially easier to observe and reverse than a misplaced dependency group.

## 3. The unit of organisation

The unit must be chosen before a relocation decision. Candidate units include an individual document, a Git worktree, a media project, a VM image with sidecars, a model bundle or a directory tree owned by another application. The system should distinguish a *membership* relationship from a *must-move-together* relationship. Two files in the same virtual collection are not necessarily a physical group.

Group discovery can combine explicit owner declarations, application manifests, repository markers and bounded model suggestions. Reading a manifest is content access and needs its own grant. A filename alone may justify a tentative relationship, not a dependency guarantee. Where dependency evidence is incomplete, prefer a virtual view, ask the owner, or keep the group in place.

The reference allocator assumes disjoint groups supplied by its caller. It does not discover dependency groups. A native implementation must reject overlap or consolidate connected groups before computing relief; otherwise a shared asset could be counted twice.

## 4. Learning architecture and evidence hierarchy

The current weighted Naive Bayes student is a real baseline. Its value is not state-of-the-art accuracy but a transparent, cheap mechanism for testing the feedback loop. A later system should compare this baseline with an embedding-based nearest-neighbour model, a small linear or tree model over contextual features, and a pairwise preference ranker. Do not add a model merely because it is more sophisticated.

Human decisions, user-authored policy and model-produced suggestions are distinct event types. A teacher's output is weak supervision. A human accepting a teacher's wording is still not proof that every inferred feature or rationale is correct. Store what the person actually chose, its context and the model version that produced the suggestion. Avoid copying a model rationale into a field labelled human explanation.

The teacher should be queried when there is an actionable ambiguity: a new project type, contradictory examples, an unfamiliar application bundle or a classification that influences an expensive decision. Repeated low-value file events should not trigger model calls. The student can abstain. The product should treat abstention as an informative state, not an error to hide.

A scalar called “confidence” is inadequate. Present model support, estimated uncertainty, novelty, evidence freshness and constraint status independently. Calibration means correspondence between reported probabilities and outcomes; it needs a separate evaluation. Guo et al. provide a primary example of why confidence quality must be tested rather than presumed [V17]. That paper does not establish calibration of Loomward's classifier or of an LLM's self-reported score.

## 5. Choosing when to ask

A useful question can improve several future decisions. A useless question interrupts the user to resolve an item they will never see again. The proposed selector ranks a question by expected decision value, reuse across a group, uncertainty reduction and the owner's recent interruption burden. It subtracts estimated attention cost and privacy exposure.

An illustrative objective is:

`question_value = expected_future_decision_improvement + immediate_regret_avoided - attention_cost - disclosure_cost`

These terms are not supplied by an LLM's confidence number. Initially use interpretable proxies and measure whether they predict useful corrections. For example, asking whether a whole client folder follows a Finance convention may be more reusable than asking about one unusually named invoice. Questions should present a small number of concrete alternatives plus “keep current” and “not sure.”

Maintain an interruption budget per profile. Batch related ambiguities, honour quiet hours and pause learning prompts during foreground work. Repeating a dismissed question should require new evidence, a changed context or an explicit revisit interval. Dismissal is not a negative class label unless that was the user's stated meaning.

## 6. Learning about use without manufacturing a surveillance product

Recent modification, recent opening, active ownership and future planned use are different signals. File timestamps can be affected by tools and backups. A journal of changes is not a complete use history and is not a reversal log [V14]. The proposed heat model therefore separates source observations rather than reducing every timestamp to one score.

Start with explicit pins, current project selection, cooperative activity leases and owner-declared schedules. Optional usage observation needs a clear privacy scope and a retention policy. Prefer aggregate counts and coarse intervals over recording command lines, every opened file or complete interaction history. Unknown use should remain unknown; it is not evidence that an item is expendable.

A forecast can estimate the probability of use over a chosen horizon and the cost of a slow recall. The decision should combine those estimates with movement cost and safety constraints. Do not train a model to maximise “bytes deleted” or “RAM freed.” Such labels reward the wrong outcome.

## 7. A proposed decision objective

For a feasible proposal p, consider:

`utility(p) = retrieval_gain + pressure_relief + foreground_benefit - transfer_cost - recall_cost - disruption - attention - risk_cost`

Not all terms are commensurable or safely estimated. Early releases should use lexicographic or constrained multiobjective decisions rather than pretend that one learned score can price data loss against convenience. Protected paths, unsupported metadata fidelity and absent authority are hard exclusions. Expected utility ranks only proposals that survive those checks.

The current allocator uses a narrow lexicographic objective: unmet free-space target, transfer bytes, number of moved groups, source-byte-weighted heat and stable IDs. It has no learned use forecast. It is therefore an operational baseline against which richer models can be evaluated, not an implementation of the full utility expression.

## 8. Six falsifiable hypotheses

**H1: Semantic views reduce physical disruption.** Compared with a folder-only workflow, virtual collections should reduce retrieval time without increasing unwanted relocations. Failure would be similar or worse retrieval time, excessive collection clutter, or a continuing need to move the same files manually.

**H2: Selective teaching improves the cost-quality frontier.** A student plus selective teacher should reach comparable accepted-suggestion quality using fewer teacher requests and fewer owner corrections than a teacher-on-every-item policy. Evaluate model token cost, latency, energy proxies and abstention coverage, not accuracy alone.

**H3: Group-aware planning avoids broken workflows.** Dependency-aware groups should reduce broken references and failed recalls relative to per-file placement in a disposable fixture environment. Test ambiguous and overlapping groups; an algorithm that improves only perfectly annotated fixtures has limited evidence.

**H4: Shared resource evidence prevents redundant work.** Cooperative admission should reduce overlapping index/hash/model work and foreground degradation compared with independent background workers. Measure the same task completion workload, not merely a lower count of running processes.

**H5: Evidence-aware review improves decisions.** Showing provenance, unknowns and recovery conditions should improve the rate of correct review choices relative to showing a confidence score alone. Increased review time may be an acceptable cost only if the reduction in wrong choices is meaningful.

**H6: A scoped interoperability layer reduces privacy exposure.** Task-specific views should reduce unnecessary metadata transmitted to an agent compared with exporting an entire catalogue. This must preserve answer usefulness; withholding so much context that every answer is wrong is not a success.

## 9. Evaluation programme

Phase A uses deterministic fixtures. Keep the old greedy counterexamples, known hash collisions in truncated fingerprints, Unicode/path cases, overlapping dependency groups, stale leases and ambiguous metadata. These evaluate mechanics, not personal usefulness.

Phase B is opt-in replay of local histories. Hold out later time windows and whole projects. Randomly splitting files from the same project between training and testing leaks naming conventions and repeated artefacts. Report per-owner and per-project results, abstention coverage, accepted suggestions per review minute, and error severity. Keep synthetic seed data separate from evaluation data.

Phase C uses shadow recommendations on a real workstation. The app records proposals and their estimated costs but never applies them. The owner labels a sample with reasons. Determine whether predictions remain useful when observations are partial and fresh content is withheld. Distinguish no response from rejection.

Phase D is a small, explicitly authorised live study of reversible virtual organisation. Freeze models for compared periods, or predefine model-update boundaries, so one arm does not benefit from labels acquired by another without accounting for that carryover. A switchback or N-of-1 design may be practical, but the analysis must consider time trends, learning effects and task difficulty. No sample-size or statistical-power claim is justified before a pilot estimates variation.

Only after native identity, recovery and consent gates pass should physical placement enter a live study. Initial trials should use disposable copies, then explicitly approved low-risk groups. There is no justification for destructive exploration in order to learn a policy.

## 10. Measurement and reporting

Primary outcomes are successful retrieval time, useful accepted proposals per unit of review effort, time spent under measured resource pressure, foreground task latency, and successful verified recovery. Guardrail outcomes include unexpected effects, wrong-scope exposure, broken references, missed cancellations, repeated unwanted prompts and app self-overhead.

For classifiers, report coverage and error together. A model can have low error by abstaining almost always. For placement, report feasibility, objective gap where known, transfer cost and the assumptions used to estimate source relief. For resource coordination, report throughput and foreground impact against the same incoming workload. For interoperability, report returned fields, payload size and answer usefulness under each scope.

Record uncertainty intervals when data supports them, and keep negative results. The first pass found nine missed feasible targets in 51 tiny scenarios. The second pass fixes those observed failures, but it does not turn a 51-case experiment into a universal guarantee.

## 11. What could make the thesis fail

The system may require more manual correction than the organisation it replaces. A useful taxonomy may shift faster than the model can adapt. App-specific dependency boundaries may be too expensive to discover reliably. The catalogue may become another heavyweight background service. Users may prefer a few explicit workspace profiles to inferred behaviour. A more capable LLM may still provide worse value than a small model for routine local tasks.

Each is a legitimate result. The architecture preserves a useful observation workbench even if ambitious learning is not justified. The long-term product should earn additional autonomy through measured utility and recovery evidence, not through a roadmap commitment to automation.

## 12. Contribution and limits

This package contributes a coherent proposed architecture, reference contracts, executable baselines, a testable protocol surface and a reproducible improvement over an identified allocation failure. It does not establish scientific novelty, field performance, usability superiority, secure execution or production model quality. A publishable thesis would require a related-work review, clearly defined datasets, preregistered comparisons where appropriate and independent reproduction of its empirical claims.

## Source references

- [V14] Windows change journal records: https://learn.microsoft.com/en-us/windows/win32/fileio/change-journal-records
- [V17] On Calibration of Modern Neural Networks: https://proceedings.mlr.press/v70/guo17a.html
