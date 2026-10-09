# Efficiency: do less work, bound the work that remains

## The optimisation order

First avoid unnecessary work, then reuse valid results, then reduce data movement, then optimise algorithms, and only then parallelise or tune low-level code. Parallelising redundant full scans is not an efficient architecture. An always-on LLM should not be the dispatcher for routine file events.

The most important future optimisation is an incremental native catalogue with visible coverage. The current Python scanner and full-state browser route are still the original bounded reference. The new SQLite snapshot catalogue demonstrates paged query mechanics; it does not turn the original scanner into an NTFS index.

## Query and IPC costs

Use stable, indexed order and keyset pagination for deep pages. Scope and disclosure precede aggregates and cursor creation. Cache only projections keyed by scope/grant, catalogue generation, query and schema. Do not cache a private result under a query-only key and return it to another caller.

Return summaries and page references, not full catalogues, to a model host. Keep tool lists small and deterministic. Four semantically meaningful tools can be more useful and cheaper in context than hundreds of file-specific tools. Structured output should have a bounded explanation, not an unbounded reasoning transcript.

Avoid one IPC request per row. Batch feature queries, use explicit projection fields and fetch detail only when the user opens an inspector. Changes can invalidate a scope or generation rather than push every modified filename to every consumer.

## Measured reference experiment

The recorded experiment constructs 50,000 synthetic metadata records in an in-memory SQLite view. It measures index construction, a 50-row first page with projection/cursor work, and equivalent SQL rows at a known deep cursor versus OFFSET. The exact results and platform are in `evidence/v2/benchmarks.json`.

The deep-page comparison excludes cursor acquisition, API serialization and filesystem I/O. This is intentional: it isolates the cost of retrieving a known page position, not end-to-end user speed. The full synthetic input and returned page payload sizes are also recorded. A small page is not equivalent to delivering the entire dataset; the payload comparison measures how much a bounded answer transmits, not compression of the same information.

Do not generalise these values into “Loomward is X times faster than WinDirStat.” No competitor, Windows scan, physical disk, cold cache or million-file UI was benchmarked. The original 5,000-file smoke result remains separate historical evidence.

## Native scan strategy

Prioritise a quick, bounded initial picture and clearly indicate partial coverage. Schedule detailed allocation/content work only when it can affect a decision. Coalesce bursts into dirty identities or scopes, persist a checkpoint and reconcile on overflow rather than retrying every notification individually [V13, V14].

Avoid waking idle disks, following reparse points unexpectedly or hydrating cloud placeholders for a routine dashboard refresh. Skip unsupported content types and surface unknown coverage. Keep enumeration, metadata, content hashing and model feature extraction as separate budgets.

A hash pipeline should group by safe metadata filters first, then read content only under consent and an I/O budget. Partial hashes can reduce candidate sets but never prove equality. Preserve byte comparison or a clearly defined equality verification step before proposing deduplication, and still separate equality from deletion safety.

## Model efficiency

Use the current cheap student for ordinary cases. Cache a teacher answer only when model version, feature batch, taxonomy, prompt contract and consent revision match. A cached answer from a revoked scope must not be reused. Coalesce similar questions by project, not merely filename similarity.

Batch embeddings and inference during spare capacity, cap the batch, and keep cancellation checkpoints between items. Quantisation or a smaller model is an experiment to evaluate, not a universal quality-preserving optimisation. Record useful suggestion rate and resource cost together. A model that is cheap but frequently wrong can cost more in owner attention.

Do not retain full prompts, raw documents or embeddings indefinitely just because disk is currently available. Retention and derived-data invalidation are part of efficiency as well as privacy.

## UI efficiency

Migrate the storage table to backend paging and virtualised rows only after preserving keyboard/selection semantics. Move large map calculations off the main thread when measured render work warrants it. Update visible panels, not the entire application, on each telemetry tick. Pause nonessential animation and polling when the window is hidden.

The current app still performs some whole-snapshot computation and full-page rerendering. The new interface is a functional design reference, not a million-row frontend benchmark. The new modules are kept separate so future native/query integration need not rewrite every page.

## Self-overhead budgets

Proposed native targets, not measured achievements: metadata-only idle mode should perform no content reads or model loads; an idle tray should avoid periodic full scans; query payloads should be capped; optional work should yield promptly at declared checkpoints. Choose numerical RSS/CPU/latency budgets after the first Windows baseline on a declared fixture and machine.

A useful initial test budget is a 50 ms checkpoint interval for cancellable chunked work and a 100 ms target for ordinary cached queries, but those are engineering hypotheses to measure, not contractual OS guarantees. Pin the dataset, hardware, filesystem, software build, cache condition and power profile before comparing results.

## Benchmark matrix

Measure cold and warm full enumeration, incremental reconciliation after a controlled mutation set, large-directory lookup, deep paging, allocation coverage, duplicate content I/O, embedding throughput, model warm/cold load, UI input latency, cancellation latency and idle resource cost. Include NTFS, removable storage and at least one unsupported/degraded path to test truthful fallback.

For competitor comparisons, use identical roots, exclusion policy, permissions and measurement basis. Some tools use a persistent index while others enumerate on demand; report that difference rather than pretending their startup conditions are identical. An integrated companion can outperform in workflow value without winning every raw scan benchmark.

## Observability without collecting the workspace

Use low-cardinality counters for work type, provider state, refusal reason and latency bucket. Keep filenames, absolute paths, command lines, tokens and document snippets out of default traces. Optional local debug bundles need explicit projection/redaction and bounded retention.

OpenTelemetry can provide a vocabulary for spans and errors [V16], but adopting it does not require exporting data to a hosted collector. Record refusals as expected domain outcomes where appropriate rather than inflating them into unexplained crashes. Observability is evidence for diagnosis, not an alternative authority system.

## Source references

- [V16] OpenTelemetry error recording: https://opentelemetry.io/docs/specs/semconv/general/recording-errors/
