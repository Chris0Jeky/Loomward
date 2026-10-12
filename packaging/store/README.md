# Microsoft Store (EXE/MSI route) checklist

Route: submit the existing NSIS installer by URL ("MSI or EXE app"); no MSIX needed. Everything below is from
Microsoft Learn, retrieved 2026-10-12; the `ms.date` / `updated_at` of each page is given.

**Read this first: signing is not optional on this route.** The brief and the certification page call signing
"highly recommended"; the requirements page, updated 2026-08-24, says the installer and *all* its PE files **must**
be signed by a CA in the Microsoft Trusted Root Program
([app-package-requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/app-package-requirements),
`ms.date` 2022-10-30, `updated_at` 2026-08-24). Treat it as a hard gate: an unsigned preview does not go to the
Store. See [`../signing/`](../signing/README.md). Also, "submit your app only when it's finished" is a listed
certification-failure cause ([certification process](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/app-certification-process),
`updated_at` 2026-08-24), so submit a stable signed release, not a `-preview.N`.

## 1. Owner actions (human-only)

- [ ] Open a developer account: <https://storedeveloper.microsoft.com> -> "Get started for free" -> **Individual**
  (the Learn page, `ms.date` 2025-09-17, `updated_at` 2026-07-17, says there is no registration fee on this flow).
  It needs a Microsoft account and government-ID plus selfie verification. Individual is for non-business
  distribution; a Company account needs a DUNS number or business documents, and Individual cannot later be
  converted to Company ([open-a-developer-account](https://learn.microsoft.com/en-us/windows/apps/publish/partner-center/open-a-developer-account)).
- [ ] Reserve the app name "Loomward" in Partner Center ([reserve-your-apps-name](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/reserve-your-apps-name)).
- [ ] Decide the publisher display name (an individual account shows the owner's name; the installer's own
  publisher string is "Loomward contributors").
- [ ] Complete the IARC age-rating questionnaire truthfully (see 4).

## 2. Package requirements vs the current installer

| Requirement (source: app-package-requirements, certification process) | Loomward today | Action |
|---|---|---|
| HTTPS direct, versioned URL; the binary behind it never changes | GitHub release asset `https://github.com/Chris0Jeky/Loomward/releases/download/v<ver>/Loomward_<ver>_x64-setup.exe`; `RELEASING.md` forbids moving a tag | The URL answers 200 after one redirect to `release-assets.githubusercontent.com` (measured 2026-10-12); the Learn pages do not say whether the Store accepts a redirect. Unverified, settle on the first submission |
| `.exe` or `.msi` only | NSIS `.exe` (no MSI: WiX rejects `-preview.N`; a stable `0.x.y` could have one) | none |
| Silent install, no UI (a UAC prompt is allowed) | `/S`, per-user, no UAC | enter `/S` under "Installer parameters" |
| Installs as a standard user; Start menu and Programs-list entry | per-user install; Tauri NSIS normally adds both (unmeasured here) | verify by hand on a clean user |
| Standalone/offline installer, "not a downloader stub" | `webviewInstallMode: embedBootstrapper` embeds the WebView2 *bootstrapper* (`MicrosoftEdgeWebview2Setup.exe` is in the v0.3.0-preview.1 installer), which downloads the runtime when it is missing (`RELEASING.md`: "fetches nothing unless WebView2 is missing") | risk: the Store may read that as a downloader. If it does, switch to Tauri's offline WebView2 installer mode (bigger installer) in `native/tauri.conf.json`. Unverified |
| Uninstalls cleanly "without leaving remnants of files, folder, and registry entries" | the uninstaller keeps `%LOCALAPPDATA%\Loomward Data\v3\` on purpose (the user's catalogue) | risk. Say so in *Notes for certification*; if rejected, add an opt-in "delete my data" checkbox to the uninstaller (an app change, owner decision) |
| Signed (all PE files) | unsigned | signing runbook; then check every PE inside the installer, including the Tauri NSIS plug-in DLLs, with `signtool verify /pa` |
| No bundled third-party software; non-Microsoft drivers/NT services disclosed | none | tick nothing |
| Package version numbering | handled by the installer; the Store does not manage Win32 versions | each release needs a new versioned URL and a new submission |

**Re-signing.** The Learn pages for the EXE/MSI route say nothing about Microsoft re-signing the file; the Store
downloads the file at the URL and certifies it, so your signature is the one users see. "Free Microsoft code signing
and CDN hosting" is listed as an MSIX benefit only (app-package-requirements FAQ). Whether Tauri can produce an
MSIX was not evaluated.

## 3. Submission fields ([create-app-submission](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/create-app-submission), `updated_at` 2026-08-24)

- **Availability:** markets (default all), discoverability, pricing = Free.
- **Properties:** category; "does this product access, collect or transmit personal information" (see 5);
  privacy policy URL; website (the repo); *Notes for certification* (<=2,000 chars, recommended): state per-user
  silent install `/S`, that data under `%LOCALAPPDATA%\Loomward Data` is kept on uninstall by design, that the app
  needs no account or network, and how to see something (the preview cannot open the folder picker; check what the
  stable build shows before writing this).
- **Packages:** package URL, architecture `x64`, installer parameters `/S`, language, app type `EXE`.
- **Store listing** (one language minimum): description (<=10,000), screenshots (1 required, 4+ recommended, max 10),
  Store logo 1:1 box art (required; 2:3 poster recommended), **applicable license terms (required, <=10,000
  characters)**: the full AGPL text is longer than that, so enter a short notice plus the URL of `LICENSE`; confirm
  Partner Center accepts it. Optional: short description, "what's new" (<=1,500), up to 20 features (<=200 each),
  7 keyword terms, copyright line (<=200), "Developed by". Pixel sizes for screenshots and logos are on
  [add-and-edit-store-listing-info](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/add-and-edit-store-listing-info) (not read here).
- Certification takes up to three business days, then about 15 minutes to appear. Later updates can be scripted with
  the Store Developer CLI / submission API for MSI-EXE apps ([publish overview](https://learn.microsoft.com/en-us/windows/apps/publish/), `ms.date` 2026-10-07).

## 4. Age rating

IARC questionnaire in Partner Center, first submission only; answers are shared with IARC along with the publisher
name and email, and the rating is then reused for updates ([age-ratings](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msi/age-ratings),
`ms.date` 2024-12-05, `updated_at` 2026-03-06). Loomward has no user-generated content, chat, purchases, location or
sharing, so a low rating is the expectation (a hypothesis; IARC decides). Answer for what the app does, not the
README's aspirations.

## 5. Privacy policy

Store Policies 10.5.1 (v7.20, `ms.date` 2026-09-14): products that access personal information need a privacy
policy URL, and "Product types that inherently have access to Personal Information must always have privacy
policies. These include ... Win32 products" ([store-policies](https://learn.microsoft.com/en-us/windows/apps/publish/store-policies)).
So a policy URL is needed even though Loomward collects nothing. Facts it can state, from `.github/release/preview-notes.md`
and `AGENTS.md` invariants 2 and 3: nothing is sent anywhere (no telemetry, cloud, teacher model or model
downloads); it reads metadata only (names, sizes, times, file identity) of folders the user grants, the installed
applications list, known-folder locations and process/disk counters; state stays in `%LOCALAPPDATA%\Loomward Data`;
the only network use is the installer fetching Microsoft's WebView2 runtime when missing. The answer to "does the
product access personal information" is a judgement for the owner: local file names can be personal data even when
nothing leaves the machine. A short `docs/PRIVACY.md` in this repository, linked by its GitHub URL, is enough; it
is not written here because it is a public statement the owner should approve.

## 6. Pre-submission test

Windows App Certification Kit is recommended by Microsoft (certification FAQ); run it on the signed build, install as
a standard user on a clean profile, uninstall, and inspect leftovers.
