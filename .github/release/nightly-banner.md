> [!WARNING]
> **NIGHTLY: an untested build.** Loomward {{VERSION}} was built automatically from commit `{{SHA}}`
> after CI passed on `main`. Nobody has tried it by hand, and it may break. Your state under
> `%LOCALAPPDATA%\Loomward Data\v3\` stays where it is; a build that changes its schema copies
> `state.db` first (`state.before-v*.db`). The nightly before this one is kept on the
> [`nightly-previous`](https://github.com/Chris0Jeky/Loomward/releases/tag/nightly-previous) pre-release,
> so the previous installer is one download away; rolling back also means restoring the matching
> state copy (RELEASING.md, Nightly). For tested builds use the preview channel.
