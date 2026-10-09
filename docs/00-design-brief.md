# Loomward: design brief

Date: 9 October 2026. Status: proposed product architecture; the prototype is an observation and simulation build, not an autonomous system utility.

## The request, preserved
Build an open-source, Windows-first companion that learns how its owner wants files, folders, shortcuts, desktop spaces and backups organised; explains disk usage; finds duplicate and potentially unnecessary data; balances files across storage tiers; and makes resource/process management understandable. Use a Rust engine where safety and efficiency matter, and machine learning rather than a pile of routing rules. A reasoning LLM may propose supervised examples and alternatives, but the user retains informed control. Preserve research, explorations, specifications, first implementations, issues and a complete local-agent handoff.

## Intended outcome
A person can answer: What is taking space? What belongs together? What should stay fast? What can be archived safely? What is putting pressure on the machine? What will this change, and how would I recover? They can make those decisions without writing terminal commands. The system becomes more useful as the person corrects it, not merely more autonomous.

## Assumptions for this pass
- Windows 11 x64 is the first release target. ARM64 is a compatibility milestone, not claimed support.
- Local-first, single-user, no account, no telemetry by default. Do not assume any particular disk letters, installed model, drive inventory, storage availability or administrator rights.
- Start as a companion to Explorer, not a replacement Windows shell or kernel driver.
- Files are not evidence of permission to read their contents. Metadata inventory, content hashing, text extraction, cloud requests and modification are separate grants.
- An empty training set is normal. Learning must abstain, not pretend to know.
- The current session is authorised to create the project and prototype without iterative design approvals. All product assumptions remain editable.
- The open-source intention does not require making an unfinished repository public. Bootstrap defaults to a new private repository; publication is a separate owner action.

## Decomposition
1. Inventory and observability: bounded scans, identity, incremental updates, storage map, process observations.
2. Personal organisation: taxonomy, virtual collections, feedback, teacher proposals, light student models.
3. Physical placement: file-group residency and disk-tier planning.
4. Action safety: capabilities, preconditions, consent, transactional operations, recovery.
5. Protection: backup providers, verified restores, retention and integrity.
6. Workspace and resource experience: desktop panels, shortcuts, process/workload controls, accessibility.

## Approaches considered
A. An LLM that invokes file and process tools directly. Fast to demonstrate, but privileges and model uncertainty become coupled. Reject as the execution architecture.

B. A deterministic organiser with optional natural-language commands. Predictable but fails the central learning objective. Retain deterministic checks for safety, not as the entire intelligence layer.

C. A local learning system that proposes typed decisions, with an independent optimiser and a capability-limited executor. More interfaces and evaluation work, but can improve without granting the model arbitrary control. Select this approach.

## First implementation boundary
Build an executable Python reference workbench with a real metadata scanner, explicit bounded duplicate hashing, a constrained storage-plan simulator, provenance-weighted supervised classifier, strict optional local-LLM teacher adapter, persistent feedback and audit records, optional read-only process telemetry, and a browser UI. Build a Rust workspace with the intended native interfaces and core implementations/tests for local compilation. A minimal Tauri host is a packaging scaffold, not a validated native application.

The current container has Python and Node, but no Rust compiler; outbound toolchain download could not resolve its host. Do not claim cargo tests, a Windows binary, Windows API verification, or native performance results. Preserve that limitation in the validation report and handoff.

## First-build prohibitions
No real file moves, deletion, renaming, hard-link replacement, shortcut rewriting, process termination, priority changes, app uninstall, registry edits or scheduled tasks. The simulator may change only simulated state. Feedback/model/audit files are application state, not user-file actions. Hashing is explicitly opt-in and may affect filesystem access timestamps.

## Success criteria
The delivered archive runs without a model, does not scan implicitly, makes its data origin obvious, preserves every decision and unknown, supports reproducible tests, and contains a path from the reference implementation to a Windows-native prototype. Product success is later measured by accepted useful recommendations, harmful-action rate, undo/recovery reliability, resource overhead and time saved. It is not measured by the number of automatic moves.
