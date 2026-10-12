> [!WARNING]
> **NIGHTLY: an untested build.** Loomward {{VERSION}} was built automatically from commit `{{SHA}}`
> after CI passed on `main`. Nobody has tried it by hand, and it may break. Your state under
> `%LOCALAPPDATA%\Loomward Data\v3\` stays where it is; a build that changes its schema copies
> `state.db` first (`state.before-v*.db`). The nightly before this one is kept on the
> [`nightly-previous`](https://github.com/Chris0Jeky/Loomward/releases/tag/nightly-previous) pre-release,
> so the previous installer is one download away. Rolling back means always moving `catalog.db` (with
> its `-wal` and `-shm`) aside, since an older build can misread a newer one without refusing it, and
> restoring the matching `state.before-v*.db` copy if the older build refuses `state.db`; the older
> build then needs a re-scan of each root (RELEASING.md, Nightly). For tested builds use the preview channel.
