# Desktop, shortcuts, applications and semantic workspaces

## Do not begin by replacing Explorer
A companion dashboard and virtual collections are lower-risk than replacing the desktop shell. Start with an ordinary app window, search/command palette and optional taskbar/tray presence. A future desktop canvas can display project groups and shortcuts without relocating the underlying files. Keep a clear exit route back to Explorer.

A virtual collection is an application query or explicit membership set. It does not need a physical folder. Examples include “active project,” “recent downloads needing review,” “documents labelled Finance,” “large cold material on fast storage,” and “working set for this week.” Multi-membership avoids the false choice of which one folder a cross-cutting document belongs in.

## Known folders and OneDrive
Resolve Desktop, Documents and related locations using Windows known-folder mechanisms [R14]. The user's actual Desktop may be redirected or cloud-backed and is distinct from the public desktop. Do not hardcode an English path or recursively reorganise every item visible on a composite shell surface. Detect known-folder ownership and provider state before proposing a physical change.

Keep personal files, app-created shortcuts, public shortcuts and virtual/system shell items separate. Restoring icon layout should not mean rewriting shortcut targets. Multi-monitor layouts, scaling, icon grid positions and accessibility require their own Windows UX tests; they are not represented by the current browser sidebar.

## Shortcut health
Parse supported `.lnk` fields through a documented adapter without executing the target. A shortcut can contain target identity/path, arguments, working directory, icon location and other metadata [R15]. Verify safe metadata inspection; do not load arbitrary icon handlers in the engine. A missing target may mean a disconnected disk, unmounted library or unavailable network path, not a broken shortcut to delete.

Potential actions: group duplicate launch shortcuts, show unavailable targets, create a new managed shortcut for an approved relocated document, or propose repair after finding an unambiguous target. Never replace a shortcut's arguments based solely on a model's guess. Inspect internet shortcuts as untrusted links and do not automatically visit them.

## Applications and unused-state evidence
Application inventory should use appropriate package/registry/provider mechanisms with documented visibility. Avoid Win32_Product enumeration because of its possible MSI consistency/repair side effects [R21]. Display system/per-user installs, app ownership, publisher, known installation location and evidence source without calling the inventory complete when providers lack coverage.

“Unused app” is an inference with a time window and coverage. Installation date, old binaries or a large directory are not proof. Portable apps, background utilities, drivers and apps used only occasionally complicate classification. Start with “not observed in this window” and ask. The process view alone cannot establish a complete launch history.

Uninstall proposals must hand off to the registered/supported uninstaller under explicit user control. Treat uninstall commands as untrusted structured provider data to verify, not text for an LLM to modify. Driver packages, system components, shared runtimes and application-managed content are excluded from a generic cleanup flow.

## Developer-aware providers
Detect project roots and preserve group integrity. Potential providers cover Git worktrees, build artifacts, language package caches, downloaded installers, model bundles, media editing projects, game libraries and virtual disks. “Recreatable” needs evidence: a manifest/lockfile, reproducible process, necessary network/credentials and acceptable rebuild cost. Do not label source changes, untracked files or local datasets as caches.

For WSL/container/VM storage, first inspect using supported management interfaces and explicit read scopes. Shrinking or moving a virtual disk is a dedicated future operation with shutdown/consistency requirements, not a generic move of a large `.vhdx`. No such action is implemented in the reference.

## Workspace sessions
A later session view may let the user pin a project, stage a working set on a fast device, surface related shortcuts and apply reversible policies to workloads launched by Loomward. Session end proposes archiving/cooling rather than doing it blindly. Calendar/task integrations are optional providers; this standalone app must not require the user's other software estate to function.

An integration with an existing local agent stack can exchange read-only inventory summaries, proposals and user-approved feedback through versioned APIs. Do not let an external agent's reasoning bypass Loomward's own consent and native safety boundaries. One authoritative plan/receipt format is preferable to multiple agents moving the same files independently.
