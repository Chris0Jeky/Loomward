# Experiments

Run `python experiments/reference_smoke.py` from the checkout. It creates only disposable synthetic files and performs a bounded exhaustive comparison on tiny synthetic tiering scenarios. It prints aggregate JSON and never inspects the user’s profile, calls an LLM, moves user files or modifies process policies. Numbers from the initial run are stored in `evidence/synthetic-experiments.json`; interpretation and limitations are in `docs/22-experiment-findings.md`.

The programme in `docs/18-opportunities-experiments.md` is future work, not a set of completed experiments. The current smoke/oracle script does not prove Windows performance, model quality or action safety.

The [reclaim union experiment](../docs/research/reclaim-union-experiment.md) is entirely
in memory: `python experiments/reclaim_union.py --need 16` prints one fictional,
read-only inspection set with per-volume relief intervals, hard-link and ancestry
accounting, and a budgeted-search certificate. It accepts no filesystem paths.
Run its independent tiny oracle with
`python -m unittest discover -s tests -p test_reclaim_union.py`.
