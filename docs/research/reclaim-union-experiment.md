# A bounded, exact reclaim-set experiment

This standalone experiment answers: **given requested relief on specified volumes,
which ancestry-compatible set of fictional candidates best meets it under the supplied
facts?** It is an inspection aid with no scanner, path input, catalogue connection,
production import, model dependency or file-effect capability. The executable lives in
[`experiments/reclaim_union.py`](../../experiments/reclaim_union.py). The reproducible
[inspection specimen](../../evidence/v3/reclaim-union-specimen.md) contains the chosen set,
its per-object facts, unknowns, excluded choices and the ranking comparison.

Meaning, residency and permission remain separate. Candidate descriptions are semantic
context; a pin is a selection exclusion; neither a score nor a selection is permission.
The counterfactual is that all selected names stop retaining their objects. Sending
them to a same-volume Recycle Bin or renaming them gives **zero immediate relief**.
No operation is offered or implemented.

## Relationship to the existing product

This builds on the supplied models, rather than changing their contracts:

- [`reclaim.rs`](../../crates/loomward-engine/src/reclaim.rs) deliberately allows nested
  candidates and refuses to sum them. That is correct. Its within-group ordering is
  verified bytes descending, then allocated bytes descending, unknowns last, then ID.
- [`placement/mod.rs`](../../crates/loomward-engine/src/placement/mod.rs) supplies the
  ancestry-antichain idea and the `(volume serial, file ID)` identity rule. Its
  `ReliefTally` requires every link and its matching allocation observation. It rejects
  sharing between placement groups; this experiment instead evaluates the combined set.
- [`planner_v2.rs`](../../crates/loomward-core/src/planner_v2.rs) supplies the bounded
  exact-search and explicitly scoped optimality pattern. Its additive disjoint groups
  cannot represent cross-candidate link complementarity. No generic optimizer is added.
- [docs/51](../51-research-programme.md) already reconciles the storage research;
  [docs/52](../52-reclaim-planner.md) remains the production reclaim contract.

Python here is experiment code, separate from the frozen `python/loomward` reference.
All its tokens and quantities are fabricated. It neither reads nor updates production
state. Existing native, copy, model-runtime and hardware gates are unaffected.

## Formal model and assumptions

Each object `o` has key `(volume, file_id)`, logical size `L_o`, observed allocated
bytes `A_o` (possibly unknown), residency `R_o` (true, false, unknown), a nonempty set
of distinct observed name tokens `N_o`, and total link count `K_o` (possibly unknown).
The same file ID on different volumes denotes different objects. Names are unique
tokens, never paths. `A_o` refers to resident allocation on this volume, not guest
free space or a content-size estimate. A nonresident object may report zero or unknown
allocation, never a contradictory positive value.

Candidates have one volume, a set of names, optional parent candidate and a meaning
label. Parent chains must be acyclic, same-volume, and contain every descendant name.
Overlapping names without a declared ancestry relation are refused. Different names
of one hard-linked object may occur in unrelated candidates. Incomplete/stale coverage,
unverified identity and explicit selection exclusions make a candidate ineligible.
This is refusal to model that candidate, not a claim it has no bytes.

For an admissible antichain `S`, let `D(S)` be the union of its names and
`m_o = |N_o intersect D(S)|`. Define per-object bounds:

```text
g_o(S) = A_o when K_o = m_o, R_o = true and A_o is known; otherwise 0
u_o(S) = A_o when m_o = |N_o| and (K_o = m_o or K_o unknown)
                 and R_o is not false; otherwise 0
```

Unknown `A_o` in the upper expression means no finite upper bound, rendered as
`null` in JSON and `unbounded (unknown)` in relief columns. It never becomes logical
size: cluster rounding, sparse data and compression preclude that substitution.
A known retained name or known extra external link gives zero upper relief for that
object. An unknown link count admits both a world with no extra link and a world with
an unseen external link: zero lower, possibly positive upper. Unique logical and
allocated bytes are displayed separately from relief, once per identity.

Sum `g_o` and `u_o` **within each volume**. The request is a vector `need[v]`, not a
pool in which spare space on another volume cancels a shortfall. The exact objective,
in priority order, is:

1. Minimize `sum_v max(0, need[v] - guaranteed_relief[v])`.
2. Minimize number of selected candidates (a transparent inspection-cost proxy).
3. Minimize unique logical bytes touched.
4. Break ties by sorted candidate IDs.

This is not a learned utility or risk score. There is no claim these tie-breaks capture
human value, recovery cost or disposability. A zero lower bound describes what can be
proved from these facts, not what physically exists.

The interval is conditional on a coherent, unchanged synthetic observation, distinct
non-shared allocation extents between object identities, no retained snapshots or open
handles, and final release of selected names. These assumptions are **not** established
by a native run. The model omits metadata overhead, alternate streams, filesystem
deduplication/clones, delayed deallocation, filesystem reserves, reparse traversal,
virtual disks, actual tool cleanup semantics and concurrent writers. Unknown identity
is excluded because an unverified key cannot establish a physical union. Possible
upper bounds may be loose; they are never an achievable promise.

## Search and its certificate

At most 12 candidates, 128 objects, 512 names and four requested volumes are admitted.
Inputs reject negative/noninteger byte counts, booleans as integers, duplicate identities
or names, contradictory counts/ancestry, and path-like IDs. Per-value bytes are u64-sized;
Python integer sums stay exact even beyond u64. There are no external representations
or filesystem observations to trust in this experiment.

An antichain-aware prefix of the current within-group ranking seeds the incumbent,
compared with doing nothing. This prefix is a **hypothetical consumer**, not behavior
attributed to the product. The solver explores include/exclude choices with a deterministic
node budget (0..200,000), never a timer that depends on machine load.

At a frontier node, the union of the selected and all remaining eligible candidates
ignores ancestry and relaxes the selection constraint. Guaranteed union relief is
monotone in names: adding names cannot lose a fully covered object. Therefore this
relaxed union gives an upper bound on guaranteed relief and a lower bound on achievable
shortfall below that node. A node is pruned only if this shortfall strictly exceeds the
incumbent's. Equal-shortfall branches still search the count/logical/ID tie-breaks.

When the budget stops, return the incumbent plus:

```text
minimum total guaranteed shortfall in
  [min(incumbent shortfall, all remaining frontier lower bounds), incumbent shortfall]
```

This interval certifies only the first objective component. A zero-width interval does
not prove count/logical tie-break optimality. `optimal_in_model` is true only when the
frontier is empty. The separate optimistic relief ceiling is the possible relief of
all eligible names, ignoring ancestry. It is safe but need not be tight. A finite ceiling
below any volume's target proves even the optimistic target impossible. Otherwise a
positive shortfall lower bound means supplied facts cannot guarantee the target;
if the bound still includes zero and no target witness exists, search is incomplete.

## Hand-derived cases and runnable proof

- **Nesting:** a 12-byte parent contains a 10-byte child. Adding individual figures
  suggests 22 bytes; the union is 12 and selecting both violates the antichain. A
  20-byte target is impossible, with exactly 8 bytes minimum shortfall.
- **Complementarity:** a 12-byte object has one name in a project and one in a cache.
  The project also contains a separate 4-byte object. Individually the existing
  all-or-unknown rule cannot establish either candidate's full relief; jointly they
  guarantee 16. The two names count one allocation, not 24 bytes.
- **Ranking:** add a standalone 7-byte candidate. The within-group ranking starts
  with that verified candidate. Its antichain-aware prefix needs three choices to
  reach 16; exact selection needs project + cache only. The production ranking does
  not promise to optimize sets and is not defective.
- **Unknown external link:** replace the shared object's total count with unknown.
  Even the whole eligible set guarantees only the known unique allocations. On the
  three-candidate fixture the minimum shortfall to 16 is 12, while possible relief
  is 16. The target cannot be guaranteed from the supplied facts.
- **Wrong volume:** a same-ID object on `archive` cannot satisfy any `fast` demand.
- **Unknown allocation:** logical size 1 with unknown allocation is `[0, unbounded]`,
  never `[0, 1]`. No invented finite ceiling is used to pronounce a target impossible.

From the repository root:

```powershell
py -3 -B -m unittest discover -s tests -p test_reclaim_union.py
py -3 -B experiments/reclaim_union.py
py -3 -B experiments/reclaim_union.py --need 24
py -3 -B experiments/reclaim_union.py --node-budget 0
py -3 -B experiments/reclaim_union.py --json
```

The tiny oracle independently enumerates all candidate subsets and concrete worlds
(link count, allocation, residency), subtracts removed names from actual link counts,
and sums physical allocations of objects with no remaining names. It does not call
the planner's relief, compatibility, scoring or search functions. Forty-eight small
models cover link closure/outside/unknown, zero/nonzero allocation, residency known/
unknown and four targets. Every admissible set's finite interval and the lexicographic
optimum agree; budgets 0, 1, 2, 4 and 8 enclose the oracle optimum. Additional tests
cover unknown allocation, nonresidency, retained observed links, volume isolation,
refusals, exclusions, exact large integer totals and deterministic ties. This small
grid is a regression experiment, not a scalability benchmark or a universal proof.

## Integration recommendation

Keep this unconnected to effects. If the inspection behavior is useful, the next
production slice should produce a generation-bound union of distinct observed names
and native identities from the current reclaim observation path, with the existing
identity/freshness, no-recall, bounded-observation and volume checks. Show the unknown
reasons and antichain exclusions before using the set result. A Rust port should first
match these synthetic fixtures; do not shoehorn shared objects into planner_v2's
additive groups or silently tighten the existing per-candidate ranking contract.

No new owner decision is required for this experiment. [HUMAN_TODO.md](../../HUMAN_TODO.md)
remains authoritative; in particular the open model-runtime choice stays unanswered,
and hardware/native qualification remains separate.
