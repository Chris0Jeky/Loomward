# Insight: an ML-first categoriser that says what things are

Status: **seed, 2026-10-25.** It records the owner's direction and a first design so that a lane can pick it up.
Nothing here is implemented, and nothing changes an invariant.

## 1. Owner direction (2026-10-25, in chat)

Before anything goes to the Recycle Bin, Loomward should "clearly inform the user of the nature of the things that
are being sent to the recycle bin, including some sort of machine learning piece of software that can give more
insights, not just names of files, but actual human readable info at a glance" (answer to
`loomward-archive-originals`).

On which model reads names and metadata, the owner chose **local first, hosted as fallback, with a per-request
preview of exactly what is sent**, and added:

- "I said machine learning, not LLM": cheap options that cost nothing should come first.
- The providers already set up should be reused. Hermes is a nice-to-have.
- Qwen 27B in LM Studio is the main local operator today. Smaller models that are just as effective for this work
  are worth a look.
- "Setting up a categoriser should be a fairly high priority", possibly with "more machine learning as a primary
  driver and then an llm (or similar) categoriser as something that picks up".
- R&D is welcome.

## 2. What exists to build on

- **The student** (`crates/loomward-learn`): weighted multinomial naive Bayes over path and name tokens. It learns
  from the owner's labels only (q-11), abstains when unsure, and its scores are relative and uncalibrated (invariant
  4).
- **Meaning and threads** (`crates/loomward-engine/src/learning`, `threads.meaning`): weak supervision from an LLM
  teacher, held as a separate data class (invariant 3). The teacher is being confined in a sandbox whose canaries have not passed yet (docs/41, q-5, q-8).
- **Developer clutter** (`clutter.developer`): rule-based detection of build outputs and caches with their
  providers.
- **Duplicates**: local content hashing (q-10).
- **Ask** (docs/46): one provider interface. It covers a local endpoint (LM Studio speaks the OpenAI-compatible API)
  and hosted providers, with a recorded request ledger. Per-folder grants decide what each can see.

## 3. Shape: a cascade, cheapest first

1. **Facts.** Deterministic facts with no learning: extension families, magic bytes, project markers
   (`Cargo.toml`, `.git`, `package.json`, `.lrcat`, `.sln`), cache and build-output rules, size, file-count and age
   distributions, duplicate share, EXIF and XMP fields for media, and whether the item is online.
2. **Classical ML.** A small model over those facts plus path tokens predicts a category and its confidence, with
   abstention. Candidates are gradient-boosted trees or linear models over hashed n-grams, and possibly a tiny
   embedding model with nearest-neighbour labels from the owner's own labelled examples. It is trained on the
   owner's labels (human class). Teacher labels may serve as weak supervision with their own weight, never mixed
   into the human class.
3. **Small language model, only on abstention or for the one-line summary.** A local model (LM Studio; evaluate
   models far smaller than 27B for this narrow task) turns the facts into one plain sentence: "Rust experiment from
   2023, 40 GB of build output, last touched 14 months ago, rebuildable". It receives facts and a bounded sample of
   names, never file contents unless the owner opts in.
4. **Hosted fallback.** Used only when no local model is available and the owner allowed it. Each request shows a
   preview of exactly what is sent before it goes, as the owner asked. q-5's scope and preconditions bind this stage:
   file and folder names only (no full paths, no contents, no extra metadata), and only after the broker's canaries
   pass, when the repository takes the `sensitive_data` overlay. Local requests (stage 3) follow the Ask grant rule
   (docs/46 Q3); whether they also get a per-request preview is this design's open question, not yet decided.

Every summary is shown with what it rests on: the facts that produced it and which stage spoke. A category or
summary is never an approval: the Recycle Bin effect still needs the owner's per-plan confirmation, and a learned
score never pre-approves anything (invariant 3).

## 4. Where it is used first

- **The archive-originals review.** Before a plan's originals go to the Recycle Bin, each group shows its summary,
  size, age and the verified copy's location.
- **`plan_archive` and cleanup surveys.** Agents read the categories (field-gated like every client read) to
  propose sensible plans.
- **Staging leftovers (Q1).** When a copy is cancelled or abandoned, Loomward explains what was staged and why it
  stopped, says whether a resume is possible, and only then removes its own staging (owner answer to
  `loomward-copy-staging-removal`).

## 5. Research questions (feed the deep-research run, `docs/research/deep-research-prompt-storage-2026-10.md`)

- Which features separate the categories the owner cares about? Build a labelled synthetic corpus plus the owner's
  own labels.
- Which classical model gives the best accuracy per millisecond, and how much do a few owner labels shift it?
- Which small local model writes faithful one-line summaries from facts? Compare sizes and quantisations against
  the 27B baseline on the same benchmark. "Laya", named by the owner, still needs identifying.
- Calibration: when may a score be called a probability (invariant 4)?

## 6. Lanes (proposed)

| Lane | Delivers | Acceptance |
|---|---|---|
| I1 | facts extractor and category taxonomy v1 (synthetic corpus, no real names) | deterministic tests on synthetic trees |
| I2 | classical classifier with abstention, trained on owner labels; evaluation harness | held-out accuracy, abstention rate and latency on the synthetic corpus |
| I3 | local small-model summariser over facts through the Ask provider; benchmark of candidate models | faithfulness checks (every claim traceable to a fact); latency |
| I4 | hosted fallback with a per-request preview | preview equals the bytes sent; refused without a grant |
| I5 | wire into the archive-originals review and `plan_archive` | UI and MCP tests |
