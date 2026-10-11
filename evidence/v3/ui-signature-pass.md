# UI signature pass (Svelte app)

Scope: presentation only. No change to data flow, transport calls, stores, contract types, accessible
names, roles, live regions, focus behaviour or `th[scope=row]`. `app/src/views/tiers/` was not touched.
Method: every rail view captured at 1360x900 and 390x844 in both themes on the mock transport before and after
(local, gitignored: `.loomward/ui-before/`, `.loomward/ui-after/`; the Learning review fixtures were a throwaway
mock patch, reverted, never committed).

## What the critique found

The Atlas and Explorer set the signature: serif display headings, a muted intro, brass dashed edge for permission,
dotted for unknown, calm tables. The views added in the last day each invented their own version:

- Page head: three shapes (h1 inside a panel; bare h1 plus muted line; h1 plus a `simbanner` local to two views).
- Honesty notes (simulation only, advice only, gated, stale, relative scores) were plain paragraphs, orange text or a
  per-view dashed box, so the strongest product claim read differently on each page.
- Status words (available, partial, denied, abstaining, missing) were bold text or bare text; `.warn` and `.bad`
  had no style in the inventory view, so its read-budget warning and error rendered unstyled.
- Empty states were muted prose indistinguishable from content.
- Controls: label-beside-control rows with buttons wrapping under them (Collections), three copies of the same flex bar.
- Tables: captions bold in one view, muted in another, hidden in a third; `.tablewrap` duplicated `.tbl-wrap`.

## What changed

Shared (`app/src/styles/`, commit 518cf56): tokens `--fs-xs`, `--fs-sm`, `--sp-1..5`, `--measure` (added only because
four views needed them). New patterns in `app.css`:

| Pattern | Meaning |
|---|---|
| `.lede` | what the page is; one measure (68ch) |
| `.note` | the honesty note: stitched brass dashed edge (the selvedge). `.note.warn` stale or incomplete; `.note.gated` switched off (dotted, the unknown stitch) |
| `.tag.ok / .warn / .bad` | status words, always text as well as colour |
| `.tag.sim` | a fact about what the product will not do (read-only, advice only): dashed edge |
| `.tag.unk` | unknown, not zero: dotted edge and italics |
| `.empty` | nothing to show yet: dotted unknown edge, so it is never read as a result |
| `.controls` / `.lw-field` / `.lbl` | one control row: stacked small label over its field, buttons on the baseline; `.btn.primary` marks the one action |
| `dl.cards` | short records as a grid of cards |
| `.tbl caption`, `.tbl small`, row hover | one caption, second-line and hover treatment |
| `.bad .warn .ok .small` | text colours that were duplicated per view |
| browser surfaces | `::selection`, scrollbar colour, `accent-color` and caret from the palette |

Per view:

- **Duplicates** (90a69e0): h1 out of the panel, lede, note; "Inspect a root" and "Inspection result" panels; Root field
  labelled the shared way, Start inspection primary; empty states dotted; partial or truncated results flagged in warn colour; status line toggles `.has`.
- **Learning review** (2f3ae30): note under the h1; status panel and queue panel; gated is `.note.gated`, stale model is
  `.note.warn`; "Advice only" is `.tag.sim`, "Abstaining" `.tag.unk`, reasons as second-line text, scores tabular;
  unknown status is an `.empty` block with the retry button. Copy unchanged.
- **Collections** (b349331): the sentence is split into lede plus note (same words); make and browse are separate panels;
  stacked labelled fields; unavailable member statuses are warn tags; Logical size right-aligned with its header.
  `AddToCollection` (the Explorer inspector) moves to the same controls pattern.
- **Applications and Desktop** (cab3edf, plus a follow-up renaming the table panel so it does not repeat the table
  region's name): h1 out of the panel; Read-only is a `.tag.sim`; provider coverage and known folders are `dl.cards`
  with state tags (available ok, partial warn, denied bad, unavailable unknown; shortcut health likewise); one panel per
  table; the provider-estimate and shortcut-metadata caveats are `.note`s; empty states dotted; the read-budget warning
  is a `.note.warn`.
- **Meaning weft** (2a682d5): `MeaningThread` gains a weft mark (crossing threads for a label or suggestion, a dotted stitch
  where there is none) and a semibold state line; text and `data-meaning-state` unchanged.
- **Explorer, Grants and Companion**: adopt `.lede` and `.note` (Companion's local `simbanner` is deleted; Tiers keeps its
  identical local copy until its PR lands, then it can switch to `.note`).
- Phone: panels use 12px padding at 600px and below, and first-column minimum width drops from 11rem to 7rem so tables
  scroll less.

Motion: unchanged. This is an operate surface; the only motion is the existing 220ms control transitions, and
reduced-motion and forced-colours rules still apply (`.tag`, `.note` edges carry shape, not just colour; row hover is
removed in forced colours).

## Checks

`npm.cmd --prefix app run check` (0 errors, 0 warnings), `run test`, `run build`, `py -3 scripts/test_app.py`
(209 PASS including the Atlas P9 timing gate: first paint about 2.5 ms for 2,500 nodes), `py -3 scripts/test_app.py --a11y`
(203 PASS), `git diff --check`. The impeccable detector over the changed files reported nothing.
One selector in the e2e needed no change; the first run caught a duplicate region name (the table panel's heading), fixed in the UI.

## Held back

- Tiers: switch its `simbanner` to `.note` and its status words to `.tag` after the Tiers PR merges.
- Health and Companion still carry scoped copies of `.bad`, `.ok`, `.small` and a `.tag.ok-tag`; they can drop them.
- Atlas and Observatory headers (h1 plus crumbs plus basis switch) are intentionally a different shape from the table views.
- Explorer and Review tables have no keyboard-focusable scroll region on narrow screens (Applications and Desktop do): a behaviour change, so not done here.
- `Learning review` was verified visually only against a throwaway mock fixture; the mock transport does not implement `learning.*`.
