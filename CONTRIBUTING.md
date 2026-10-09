# Contributing

Read `AGENTS.md` and the implementation status first. Start with one backlog issue and a reproducible fixture. Prefer small, reviewable changes with tests and source-linked claims. Safety, protocol, privacy, elevated privileges and any new mutation capability require explicit design review.

The reference uses Python 3.11+ with no mandatory dependencies; optional process telemetry uses psutil. Run `python scripts/verify.py`. UI browser tests require Playwright and a Chromium executable. Native compilation is pending; help establish the Rust/Windows baseline without lowering the safety boundary.

Use synthetic data in committed tests and screenshots. Do not include personal paths, credentials, model weights, database files, native build artifacts or third-party fonts. Source is MIT; independently review the licences of any new dependencies/assets. Report exact native builds and tests rather than assuming portability from Linux.
