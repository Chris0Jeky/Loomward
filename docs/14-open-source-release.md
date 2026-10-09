# Open-source project and release strategy

## Name and identity
Working name: **Loomward**. It suggests weaving a useful structure while keeping watch over changes. This is a provisional project name, not a claim of trademark/domain clearance. An exact GitHub repository search found no result at research time; that is a narrow search, not proof that the name is unclaimed. Before a public launch, check relevant registries, package namespaces, domains and potentially conflicting software brands.

The initial package includes MIT licensing for its original source. Confirm the project's final licence policy before the first public release, and keep third-party licence obligations independent. Referencing a GPL-licensed comparator does not mean its code was copied or relicensed into this package. No fonts, model weights or third-party binary distributions are included.

## Repository posture
The package is a new standalone project. It does not modify an existing repository in the user's estate. No remote repository, branch, PR or GitHub issue was created in this pass. The available connector's exposed actions did not include new-repository creation. `scripts/publish_github.py` prints a proposed GitHub CLI command by default and requires `--execute` for publication. It defaults to private. Public publication needs an additional explicit repository-name confirmation.

The local backlog uses stable IDs `LW-001` and so on; these are not GitHub issue numbers. An importer embeds an ID marker into each issue and skips already-present markers. It does not create project boards/milestones or claim multi-writer concurrency safety. Keep one importer active and review its output. Imported issues should receive actual URLs/IDs only after GitHub returns them.

## Contribution boundaries
Require focused issues, exact tests, source-linked claims and clear implementation status. A contribution adding a destructive endpoint, changing safety invariants, adding elevated privileges, enabling cloud data transfer or granting broad Tauri plugins needs explicit security/architecture review. A UI mock must never be represented as a functioning backend action.

Use separate modules for catalogue, Windows adapters, learning, planning, UI, backup providers and execution. One owner at a time for the protocol/schema files. Read-only research or tests can proceed in parallel; only an integration owner changes shared contracts and release status. Keep a clean evidence trail rather than relying on conversational memory.

## Dependency and build integrity
The authoring environment had no Rust toolchain, so native dependencies were not resolved into a lockfile and native code was not formatted/compiled/tested. The first local milestone must generate and review lockfiles, run formatting, compile, test and audit dependencies. Do not claim reproducible native builds before this exists. The workflow file is a proposal, not a successful hosted run.

Before public release, pin CI actions to reviewed full commit SHAs, minimise workflow permissions, generate an SBOM, check licences/advisories and verify dependency provenance. Sign release artifacts and update metadata through an explicitly designed release pipeline. Avoid running untrusted pull-request code with signing credentials or privileged runners. The updater needs its own trust boundary and rollback behavior.

## Privacy and support
Diagnostics are opt-in. The default report contains versions, capability failures, performance counters and redacted paths, not a user's full filename index or model prompts. A snapshot or feedback export is potentially private and must be labelled accordingly. No telemetry endpoint or cloud account is required for core operation.

Provide a security policy before a public vulnerability-reporting channel exists. Do not invent an email address that the project cannot receive. For early private testing, use repository-owner contact and avoid posting private files, keys or exploit-containing samples publicly. The prototype is not a safety-certified file utility or backup product.

## Release sequence
First publish source and the read-only reference with clear limitations. Then a native observation alpha, a teachable personal-organisation alpha, backup/restore integration, ordinary-file copy-only experiments and only later approved moves. Keep process modification on its own gate. A genuinely useful storage/learning product can ship before it becomes a full desktop/resource manager.
