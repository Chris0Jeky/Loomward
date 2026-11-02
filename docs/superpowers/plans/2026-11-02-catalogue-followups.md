# Catalogue follow-ups implementation plan

> **For agentic workers:** Use superpowers:executing-plans; each slice retains
> its negative evidence and is reviewed before integration.

**Goal:** Correct Reclaim model-store evidence and advice (#1101), then reduce
first-read duplicate counting cost and pin its cache invariants (#1104, #1105).

**Architecture:** Preserve bounded read-only catalogue queries. Pass the service's
known root staleness into model-store detection before filtering known-empty
folders. Use one Hugging Face signature predicate and bind custom-cache advice
to the detected location. Count duplicate-size groups once and retain all
eligibility, deadline, guard and revision semantics.

**Tech Stack:** Rust, SQLite, Svelte, Python measurement and browser harnesses.

**Spec:** Issues #1101, #1104 and #1105; docs/52-reclaim-planner.md;
crates/loomward-catalog/README.md.

## Constraints and review focus

- No scan, filesystem effect, migration, new dependency or permission change.
- An unobserved or stale size remains unknown, including a formerly empty folder.
- Every eligible ComfyUI cache belongs to exactly one provider; excluded signature
  rows cannot remove it from both.
- Guidance for a custom cache cannot silently target a default cache.
- Cache hits must retain live-scan, aggregate validity, root/floor and instance
  fences; interrupted work remains unknown and is never cached.

## Work

- [ ] Pin custom Hugging Face advice and stale empty ComfyUI retention through
  existing catalogue/service APIs; publish the native tests-first checkpoint.
- [ ] Correct detector inputs, signature sharing and context-specific advice;
  add grant-boundary and damaged-signature cases.
- [ ] Measure current duplicate counting with synthetic spread-size inputs;
  replace repeated eligible-set work only if count parity and work reduction hold.
- [ ] Pin cache guards and instance identity, correct the grant fixture, and
  measure the updated query separately from warm-cache reads.
- [ ] Complete applicable app/native/contract/browser checks, independent review
  and exact-head qualification; record measured scope and unavailable tooling.

Advice sources checked 2026-11-02: Hugging Face's
[cache guide](https://huggingface.co/docs/huggingface_hub/en/guides/manage-cache)
documents explicit `--cache-dir` selection for custom caches; ComfyUI's
[model-path example](https://github.com/Comfy-Org/ComfyUI/blob/master/extra_model_paths.yaml.example)
defines `diffusers` and other model locations in `extra_model_paths.yaml`.
No external tool is invoked by this work.
