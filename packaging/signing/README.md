# Code signing: how a certificate plugs in later

Today nothing signs (`RELEASING.md`, "Signing is a future owner action"). The Microsoft Store EXE/MSI route
**requires** a signature ([store checklist](../store/README.md)); winget and Scoop do not, but SmartScreen warns on
an unsigned installer everywhere. Since 2024 a certificate does not buy instant SmartScreen reputation (EV and OV
build it the same way; [Tauri: Windows code signing](https://v2.tauri.app/distribute/sign/windows/), "Last updated:
Sep 11, 2026", retrieved 2026-10-12), so expect a warning period even after signing.

## The seam: Tauri's `bundle.windows`

Tauri signs the app binary and the NSIS installer while it builds. The keys (JSON schema
`crates/tauri-schema-generator/schemas/config.schema.json`, commit of 2026-10-10):

| Key | Meaning |
|---|---|
| `certificateThumbprint` | SHA-1 of a certificate in the Windows store; Tauri then calls `signtool` |
| `digestAlgorithm` | `sha256` |
| `timestampUrl`, `tsp` | RFC 3161 timestamp server; `tsp: true` if the provider uses TSP |
| `signCommand` | a custom signer. A string (split on spaces) or `{ "cmd": "...", "args": ["...", "%1"] }`; `%1` is replaced by the file to sign. Use the object form when a path or argument contains a space |

**Keep the signing keys out of `native/tauri.conf.json`.** A committed `signCommand` would break every local
`tauri build` and the unsigned smoke runs for anyone without the signer. `release.yml` already injects the version
through a second config file (`printf '{"version":"%s"}' ... > "$RUNNER_TEMP/tauri-version.json"` then
`tauri build --config`); the signing change extends that one JSON, which Tauri merges over `tauri.conf.json`:

```sh
printf '{"version":"%s","bundle":{"windows":{"signCommand":{"cmd":"artifact-signing-cli","args":["-e","%s","-a","%s","-c","%s","-d","Loomward","%%1"]}}}}' \
  "$VERSION" "$SIGN_ENDPOINT" "$SIGN_ACCOUNT" "$SIGN_PROFILE" > "$RUNNER_TEMP/tauri-version.json"
```

(`%%1` is `printf` for a literal `%1`.) The release's `SHA256SUMS.txt` is computed after the build, so it already
hashes the signed installer. The `-d` description shows in the UAC prompt for MSI installers (Tauri doc).

## Option A: Azure Artifact Signing (formerly Trusted Signing), recommended if eligible

Managed service, certificates in Microsoft HSMs, nothing to renew by hand
([overview](https://learn.microsoft.com/en-us/azure/artifact-signing/overview), `ms.date` 2026-01-02;
[quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart), `ms.date` 2026-05-21, `updated_at` 2026-10-08).

**Eligibility (the deciding fact):** Public Trust certificates go to organisations in the US, Canada, EU, UK,
Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel; **individual developers must
be located in the United States or Canada**, and individual validation reads the legal name and address from an
Azure billing account of type *Individual* that must match the government ID. If the owner is elsewhere as an
individual, use option B or C.

Owner steps (portal only for identity validation):

1. Azure subscription and Entra tenant; register the `Microsoft.CodeSigning` resource provider.
2. Create an Artifact Signing account (Basic SKU is enough; price: check the Azure pricing page, not read here) in a
   supported region and note its endpoint, e.g. `https://wus2.codesigning.azure.net`.
3. Identity validation -> Individual -> Public (government ID via the Verified ID flow in Microsoft Authenticator).
   The role *Artifact Signing Identity Verifier* is required to start it.
4. Create a certificate profile of type **PublicTrust** from that validation.
5. Create an Entra app registration (service principal); assign it the role **Artifact Signing Certificate Profile
   Signer** on the profile ([assign roles](https://learn.microsoft.com/en-us/azure/artifact-signing/tutorial-assign-roles)).
6. In GitHub: environment `release` (with a required reviewer) holding secrets `AZURE_TENANT_ID`,
   `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` (the three variables Tauri's guide names) and plain variables
   `SIGN_ENDPOINT`, `SIGN_ACCOUNT`, `SIGN_PROFILE`. Never paste the secret into a chat or a file in the repository.
7. Tell the agent the three non-secret names; the workflow edit below is then a small PR.

The CLI is installed in the job with `cargo install artifact-signing-cli --locked --version <pinned>` (Tauri guide;
pin the version in the PR). Hypothesis to test rather than assume: a federated (OIDC) credential via
`azure/login` would avoid the long-lived client secret; the guide documents only the three secret variables.

## Option B: OV or EV certificate

Tauri's guide covers `.pfx` import (`Import-PfxCertificate`, secrets `WINDOWS_CERTIFICATE` base64 and
`WINDOWS_CERTIFICATE_PASSWORD`, then `certificateThumbprint` + `digestAlgorithm` + `timestampUrl`) only for OV
certificates bought before 2023-06-01; newer OV and EV certificates "should follow the issuer's documentation".
My understanding (CA/Browser Forum baseline, not re-read here) is that those keys now live on hardware or a
vendor cloud HSM, so a downloadable `.pfx` is not issued and a GitHub-hosted runner cannot reach a USB token. Real
options: the vendor's cloud-signing client wired through `signCommand` (vendor documentation, with its own
secrets), or Azure Key Vault + `relic` with `relic.conf` (Tauri guide; roles *Key Vault Certificate User* and *Key
Vault Crypto User*). Owner steps: buy from a CA that sells cloud-HSM signing, complete its identity vetting, give
the agent the vendor's signing CLI name and its secret names.

## Option C: SignPath Foundation (free for open source)

Free certificates for OSI-licensed projects; the publisher shown is **SignPath Foundation**, every release needs
manual approval, builds must be reproducible from the repository, and the project must publish a code-signing policy
page ([terms](https://signpath.org/terms), undated draft, retrieved 2026-10-12). The terms exclude "commercial
dual-licensing"; Loomward is AGPL-3.0-only with a CLA that keeps a relicensing right with the Maintainer (PR #396),
so eligibility needs the owner's reading before applying. Different publisher name from the owner's Store account.

## The agent's PR once secrets exist

1. `release.yml`, job `windows-preview` (job name will want to drop "preview" for stable releases): add
   `environment: release`; install the signer; build the second config as above; add
   `SIGN_*`/`AZURE_*` to the `Build the NSIS installer` step's `env:` only; keep `permissions` unchanged
   (add `id-token: write` to that job only if OIDC is adopted).
2. Check signatures where the files exist: `Get-AuthenticodeSignature` on the installer right after "Collect the
   installer", and on the installed `loomward-desktop.exe` and `uninstall.exe` inside the "Smoke-test a silent per-user
   install and uninstall" step, after installing and before uninstalling. Fail unless `Status` is `Valid` and the
   subject is the expected one. `signtool verify /pa /all <file>` also works on the runner.
3. Retire "unsigned" wording in lockstep: `RELEASING.md` (installer row, "Signing is a future owner action"),
   `.github/release/preview-notes.md` (the "Unsigned." paragraph), the release `--title` (`(unsigned preview)`),
   `native/tauri.conf.json` `shortDescription`, the Scoop `notes` and winget `Description`. A mismatch is a false claim
   (`AGENTS.md` invariant 9).
4. First signed release: check the PE inventory the Store wants ("all of its PE files"): extract the installer with
   7-Zip and `signtool verify /pa` each `.exe`/`.dll`; list any unsigned file (the Tauri NSIS plug-in DLLs are the
   suspects) before the Store submission.
5. Re-measure the winget and Scoop channels once with the signed build (hash changes with the signature, which the
   existing flows already read from `SHA256SUMS.txt`).

Secrets rules: only the secrets of the option the owner picks are ever needed (option A's three Azure values, option
B's certificate and password or vault credentials, option C's SignPath credentials); nothing is committed, and
`.gitignore` already blocks `*.pfx`, `*.pem` and `*.key`.

Not verified: any of this against a real certificate or Azure account; `artifact-signing-cli` behaviour (read only in
Tauri's guide); that the Tauri config merge accepts the nested object (documented behaviour for `--config`, to be
proven by the first signing PR with an unsigned dry run using `signCommand` `echo`).
