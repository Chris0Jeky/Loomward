# Personal scans opened (11 October 2026)

Measured on Windows 11, without elevation. This PR removes the `personal_scan_gated_184`
refusal now that all four personal-scan safety gates are closed: #184 (audit, #274), #206 (#225),
#240 (#252) and #269 (containment re-proved before every relist, #278). It first closes #286's
MEDIUM: the profiles directory is resolved from the OS (`FOLDERID_UserProfiles`; elsewhere the
parent of `HOME`), so a sibling profile under a redirected, differently named profiles folder is
refused, and an unresolvable profiles directory refuses every root.

## What a personal session may scan

- Its own active owner grants, from the desktop picker or `--grant-root` (origin `owner_granted`).
- Refused: a revoked or inactive grant (`permission_denied`), a lab root
  (`personal_session_requires_owner_grant`; the catalogue already refuses to register one in a
  personal store, and the scan gate refuses a row that claims one anyway), an unknown root (`not_found`).
- Metadata only: names, sizes, dates and attributes. Duplicate content reads stay gated
  (`content_read_gated`). No new effect, endpoint or elevation; an elevated token is still refused.
- A synthetic session is unchanged: registered lab roots only.

## Tests

| Check | Result |
|---|---|
| `personal_session_scans_a_granted_folder_metadata_only` (service) | a folder granted through the CLI path scans to completion with complete coverage; totals match an independent walk and the fixture (4 files, 3 folders, 135 bytes); file contents and modification times are identical before and after. On an elevated host it asserts the `identity_unavailable` grant refusal instead |
| `personal_scan_start_refuses_unknown_lab_and_revoked_roots` (service) | unknown, lab and revoked roots refused as above |
| `profiles_directory_is_resolved_from_the_os_not_by_name` (service) | a sibling profile under a profiles folder with an unrecognised name is refused, as are the folder itself and a root containing it; #253's non-profile sibling of a redirected profile stays eligible; missing or unresolvable folders fail closed |
| Mutation: restore the gate | both personal tests fail (on a non-elevated host; on an elevated host such as hosted CI the scan test asserts the `identity_unavailable` refusal instead) |
| Mutation: allow a lab root in a personal session | the refusal test fails |
| Mutation: skip the OS profiles-directory check | the #286 test fails |
| `scripts/test_app.py --live-serve` | a personal `loomward-serve` granted one small folder on the command line; pressing Scan in the app reaches "Finished, complete coverage"; `roots.list` totals match the fixture; no file changed; an unknown root is `not_found`; only the pressed Scan created a job |

## Real folder through the product (owner grant q-4)

`real-A (owner-authorised, agent-picked)`, the same root as `real-A` in
[real-folder-stress](../../docs/research/real-folder-stress.md). Its path was appended to the
primary checkout's gitignored `.loomward/scan-roots.log` before each scan. The release `loomward-serve
--dataset personal --allow-personal --grant-root <root>` ran with a fresh state directory under
the gitignored `.loomward/`; `scan.start` (full) was called over HTTP and `jobs.get` polled to the
end; the server was stopped afterwards. Aggregates only; no names, paths or per-file data.

| Run | State | Coverage | Files | Folders | Denied (failed) | Skipped | Logical | Allocated | Elapsed | Entries/s |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1, path as stored | completed | partial | 291,936 | 31,835 | 2 | 16 | 68.2 GiB | 68.6 GiB | 1,017 s | 318 |
| 2, path typed in lower case, fresh state | completed | partial | 291,936 | 31,835 | 2 | 16 | 68.2 GiB | 68.6 GiB | 1,039 s | 312 |

- Coverage is partial, honestly: two folders denied access, and 16 reparse or placeholder entries
  were skipped. The earlier raw enumeration of this root (10 October) also counted 2 denied
  folders and 16 skips, with 291,935 files and 31,837 folders.
- The second run answers #285 item 1 for the product path. A grant typed in a different letter
  case is canonicalised at grant time: the stored display path came back in the volume's own case,
  not as typed. The scan then completed with identical counts. This does not exercise a root
  handle opened through a differently cased path inside the native layer, which no product path
  produces.
- Throughput: about 315 entries/s, against roughly 20,000 files/s for the raw, catalogue-free
  enumeration of the same root. The gap lies in the product pipeline (per-directory listing,
  containment proof and catalogue writes); this run does not attribute it among them. The
  timings are shared-host and uncontrolled; run 1 overlapped a local test run for part of its
  time, run 2 did not.

## Not established

Personal scans on FAT, exFAT or RAM-disk volumes (#285 item 2). Desktop picker grants on a real
folder (the picker needs a human; the CLI grant path was used). Large-folder product-scan
throughput is not a target here and is far below raw enumeration.
