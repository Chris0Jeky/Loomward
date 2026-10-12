# Package channels

Owner decisions HUMAN_TODO q-28 and q-29 (2026-10-12): Loomward ships through GitHub Releases (the source of
truth, [RELEASING.md](../RELEASING.md)), winget, a Scoop bucket and, later, the Microsoft Store. The build is an
**unsigned preview** today; the owner will add a Store developer account and a code-signing certificate "in the near
future", so every channel here is written to survive that switch without a redesign.

| Channel | Lives in | State | Needs a release first | Needs signing |
|---|---|---|---|---|
| GitHub Releases | `.github/workflows/release.yml` | working | it is the release | no (SmartScreen warns) |
| Scoop | [`../bucket/loomward.json`](../bucket/loomward.json), [`scoop/`](scoop/README.md) | placeholder until the owner approves publishing; checks pass against v0.3.0-preview.1 | yes | no |
| winget | [`winget/`](winget/README.md) | generator validated on v0.3.0-preview.1; nothing submitted | yes | no, but see its risk list |
| Microsoft Store | [`store/`](store/README.md) | checklist only | yes | **yes** (the Store docs say must) |
| Signing | [`signing/`](signing/README.md) | runbook only | n/a | n/a |

Order that works: cut a preview with `release.yml` -> run `checkver` for the Scoop bucket -> generate and submit
the winget manifest -> later, add signing, then the Store.

Nothing here edits `.github/workflows/*`, `native/tauri.conf.json` or `RELEASING.md`; the signing runbook lists
the small edits those files take when the time comes.

Checked against the real pre-release v0.3.0-preview.1 (2026-10-12): Scoop `checkver`/`autoupdate`, the 7-Zip
layout, and the winget generator and validator (details in the two READMEs). Nothing is published on any channel:
the first publication of each is an owner re-review trigger (`AGENTS.md` invariant 8), so the Scoop manifest stays
a fail-closed placeholder and no winget PR exists. Not verified anywhere: an actual `scoop install`, `winget install`
or Store certification.
