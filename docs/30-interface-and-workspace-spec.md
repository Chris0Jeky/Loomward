# Interface specification: understandable control, not a chat window

## Product stance

The main interface should answer four questions: what is happening, why does it matter, what are the choices, and what will actually change? Conversation is one optional input method. It should not replace inspectable tables, maps, diffs, scopes and receipts.

The existing green/neutral visual language is retained. The prototype now has ten pages. Overview, Storage explorer, Organise & learn, Disk tiers, Processes, Activity and Protection remain; Decision desk, Connections and Resource budgets are additive. The new screens use the same typography, panels, navigation, status vocabulary and mobile layout rather than introducing a disconnected mockup.

## Information architecture

The eventual navigation can be grouped into Workspace, Decisions and System. Workspace contains search, physical trees, virtual collections, projects and workspace profiles. Decisions contains review candidates, saved alternatives, unresolved evidence and effect receipts. System contains volumes, processes, resource budgets, providers, protection and application health.

Do not create a page for every backend module. A person troubleshooting a stalled build should see memory pressure, relevant workers, current leases and the evidence behind a suggested pause together. A person filing documents should not need to understand a scheduler.

The prototype's ten pages are not the final navigation proof. Usability testing should compare this structure with a task-oriented home screen. Retain keyboard navigation, a predictable global search shortcut and deep links that identify a view without conferring permission.

## Overview and coverage

Show measured pressure separately from estimated opportunity. “44 GiB available” is an observation; “36 GiB could be relieved” is a proposal; “36 GiB recovered” requires an effect and a fresh capacity observation. Those values must never share an ambiguous badge.

The top-level state includes source, observation age and coverage. An imported snapshot should remain visibly imported on every page. A failed live connection must not silently load demo data. The original application already preserves that boundary and the new pages do not connect providers in the background.

Add a self-health card in the native release: catalogue freshness, queued reconciliation work, app resource use, last model load, last error and a clear pause. This makes it possible to hold Loomward to its own efficiency promise.

## Storage explorer

Keep the physical tree and semantic view distinguishable. A user may switch between folders, groups, file types, projects, age bands and physical allocation, but the selected basis should remain visible. Treemap size should specify logical, allocated, unique-object or estimated-reclaimable bytes. Do not combine categories measured under different bases into one total.

The current file-family map uses logical size and extension grouping. It is not a physical-allocation map or a learned semantic graph. The current browser receives a full snapshot and renders bounded rows; true catalogue-backed paging is still a next issue. The new paged catalogue provides a tested backend seam for that migration.

Future selection should support an inspector with identity observations, paths, group membership, content-read status, duplicate evidence, usage signals, recoverability and linked work. A “why excluded” view is as important as a recommendation list because it explains why the tool refuses an apparently simple cleanup.

## Decision desk

A decision card has an immutable proposal revision and an explicit evidence snapshot. Its fields are: question, current state, alternatives including keep-current, expected effect, cost, assumptions, unknowns, recovery requirement, review status and next verification. The user can compare alternatives without approving one.

The implemented desk contains three illustrative cases: virtual membership, cold project placement and foreground workload coordination. Its buttons record session-only review notes and export them. They do not update collections, write tasks, approve file operations or start workers. The UI says “No authority granted.” This is deliberately not a counterfeit approval workflow.

The native review workflow should have distinct buttons for saving a preference, requesting evidence, approving a supported manifest and running an approved action. If a proposal changes after approval, invalidate that approval visibly. Do not reuse a generic “OK” button for consent to content access, external disclosure and file deletion.

## Connections and MCP laboratory

A provider tile displays implementation state and current connection state separately. A schema or example adapter is “design,” not “connected.” The current reference displays the implemented snapshot MCP interface and unconnected Estate Console, Taskdeck, Agent Harness and backup-provider designs.

The inspector shows what the provider can read, what it can suggest and what is not granted. The request builder lets the user inspect a modern MCP request; selecting catalogue search requires an explicit disclosure example. The checkbox does not modify a live server's grant. Exporting a draft does not install a provider or send a request.

A production connection flow should show the exact fields leaving Loomward, the authenticated recipient, whether that recipient may use a hosted model, grant lifetime, revocation path and a test result. “Local-first” is not a substitute for this disclosure.

## Resource budgets

The new lab lets the user change synthetic CPU-slot, RAM, GPU and I/O budgets and compare admitted/deferred requests. Live mode runs the Python simulator; standalone mode uses a fixture-matched browser reference. It does not impose OS limits or represent the owner's actual machine measurements.

The native page should separate total capacity, measured current use, reserved incremental demand and safety headroom. It should explain every deferral in plain language, with technical details expandable. Presets are named policies, not universal optimisation recipes. A foreground profile should pause optional owned work rather than imply that other apps can be safely trimmed or terminated.

## Desktop and shortcut direction

Use workspace profiles as logical assemblies: selected projects, virtual collections, launch targets, pinned groups and an optional resource policy. A profile is not initially a replacement Windows shell. Start with launch/restore previews and explicitly selected shortcuts. Do not rearrange the desktop or rewrite shortcuts behind the owner's back.

The future shortcut inspector should distinguish a missing target, an offline volume, a moved object, a denied path and an unsupported target. A repair proposal must bind to the actual target identity and explain argument/working-directory changes. File relocation should not depend on creating thousands of junctions as an invisible compatibility layer.

Virtual-desktop layouts, shell namespace extensions and advanced Explorer integration are optional later capabilities. They should not block the first useful workbench. A kernel filesystem filter is outside the initial architecture.

## Replacing routine terminal interactions

Offer typed operation cards for supported activities: inspect a process tree, explain memory pressure, inspect a volume, simulate a placement, compare snapshots or request a worker budget. Each card exposes its inputs and evidence. Expert users can inspect the underlying command or provider contract where appropriate, but an agent must not turn arbitrary natural language into privileged shell execution.

Comprehensive process inspection could eventually include services, startup entries, handles, modules, networking and job membership. Each needs native sources, privacy limits and actual Windows tests. The current process page remains read-only basic telemetry; do not imply System Informer parity.

## Accessibility and interaction quality

All controls need accessible names, visible focus, keyboard operation and clear state. Navigation should reset content scroll predictably without losing the sidebar's usability. Use text and shape as well as colour for unavailable/blocked states. Respect reduced-motion preferences and avoid continual animated “thinking” effects in an otherwise idle app.

The interface is exercised at desktop and 390-pixel mobile width, with hostile imported filenames rendered as text. This is not a complete accessibility audit: screen-reader flows, contrast ratios, high-contrast Windows mode, localisation, large-text scaling and long-language labels still need explicit verification. Measure review comprehension with real users before calling the design intuitive.
