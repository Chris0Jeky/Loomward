# Contributing

Read `AGENTS.md` and the implementation status first. Start with one backlog issue and a reproducible fixture. Prefer small, reviewable changes with tests and source-linked claims. Safety, protocol, privacy, elevated privileges and any new mutation capability require explicit design review.

The reference uses Python 3.11+ with no mandatory dependencies; optional process telemetry uses psutil. Run `python scripts/verify.py`. UI browser tests require Playwright and a Chromium executable. Native compilation is pending; help establish the Rust/Windows baseline without lowering the safety boundary.

Use synthetic data in committed tests and screenshots. Do not include personal paths, credentials, model weights, database files, native build artifacts or third-party fonts. Source is AGPL-3.0-only (versions before 2026-10-11 were MIT); independently review the licences of any new dependencies/assets. Report exact native builds and tests rather than assuming portability from Linux.

## Licence and the CLA

Contributions are licensed to the project under [CLA.md](CLA.md), which lets the maintainer distribute them under AGPL-3.0-only and, where needed, under other terms (the project is dual licensed). You keep your copyright. Once the CLA check is enabled on the repository, a bot comments on your first pull request and asks you to sign; the check stays red until you do. Maintainer and bot accounts are exempt. Use of the project name by forks and modified versions is covered by [TRADEMARKS.md](TRADEMARKS.md).
