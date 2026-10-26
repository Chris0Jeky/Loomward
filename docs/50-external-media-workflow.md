# External media workflow: browse instantly, transfer in the background

Status: **seed, 2026-10-25.** It records the owner's direction and the open questions so that research and a lane
can pick them up. Nothing here is implemented.

## 1. Owner direction (2026-10-25, in chat)

The owner wants to make external disks "highly efficient for data browsing", for example to "pull in, clearly,
cleanly, quickly, efficiently, .RAW files (pictures) to modify in Lightroom". The bottleneck should be the transfer
speed. The workflow is:

1. decide which files to work on;
2. leave a transfer job running in the background.

Browsing, understanding where the files are and seeing what they look like should not be the pain. This likely
needs caching, indexing and "more novel approaches", with a lot of engineering and research. The owner asked to
"seed everything, understand what I need and document everything so it's ready to be picked up effectively".

## 2. The experience to build

- **Browse offline-fast.** The external disk's photos show as a fast grid, by shoot, date, camera and lens, rating
  and keywords. This works even when the disk is asleep or unplugged, because thumbnails and previews come from a
  local cache that Loomward builds once.
- **Choose with confidence.** Near-duplicate bursts are grouped, and each shoot has a one-line summary (docs/49). The
  view shows what is already on the internal disks and what is not.
- **Pull in the background.** Picking shoots queues a transfer job. The job is copy-and-verify once its gate is open
  (docs/47); until then it follows the interim path of docs/48 section 6. Progress, an estimated time of arrival,
  resume after unplug, and verification are all shown. Lightroom then imports, or relinks, the local copies.
- **Send back when done.** The edited shoot goes back to the archive disk with its sidecars, the reverse of the pull,
  under the same rules.

## 3. Pieces Loomward already has

The catalogue, scanner and offline-root semantics for the disk (docs/48 E0); copy-and-verify; duplicate hashing; the
categoriser seed (docs/49); the Svelte app and Tauri shell.

## 4. Open design questions

- **Previews.** Extract the embedded JPEG previews from RAW files (CR3, NEF, ARW, RAF, DNG) without a full decode:
  LibRaw, exiftool or the Windows Imaging Component. Consider the cache size per 10,000 photos, the eviction policy,
  and where the cache lives (an internal disk, never C: while it is full).
- **Lightroom Classic.** Choose between importing copies and relinking moved folders, deal with XMP sidecars and
  smart previews, and decide what an external tool may touch and what must go through Lightroom.
- **Placeholders.** Could the Windows Cloud Files API or ProjFS present archived folders in place and hydrate them
  from the external disk on demand? How do Lightroom, antivirus and an unplugged disk behave then? This would change
  docs/05's placeholder rules for Loomward's own placeholders: research first.
- **Throughput.** On a USB 3 HDD, measure I/O size, queue depth and ordering by on-disk extent, and the cost of
  verification. Target: transfer at the disk's sequential speed.
- **Indexing.** EXIF and XMP fields into the catalogue (a new metadata provider), perceptual hashes for bursts, and
  incremental refresh after the disk returns.

## 5. Next steps

1. Run the deep-research prompt (`docs/research/deep-research-prompt-storage-2026-10.md`, Part D) and bring the
   report back.
2. Spike: extract previews from a synthetic RAW sample set and measure time and cache size.
3. Write a design (ADR) for the preview cache and the transfer queue, then mint LW items.
