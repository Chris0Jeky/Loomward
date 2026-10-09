# Evidence interpretation

`verification.txt` is the final full Python/JavaScript/bridged-Chromium verification. Earlier `red-*` and `review-cli-test-contract.txt` preserve failing development stages, not current passing status. The final reference has 89 Python tests and 11 bridged UI checks; Rust remains unverified. `artifact-validation.txt` records JSON schema, fixture, SQL syntax and issue-graph checks. The SQL check used only a disposable in-memory database.

The browser harness used local rendering with a Python HTTP bridge. `screenshots/overview.png` and `disk-tiers.png` show synthetic demo data, not the owner’s computer. `mobile.png` intentionally shows an imported snapshot with an adversarial filename rendered as plain text. That is a security-test state, not an application error. Direct browser navigation and Windows/WebView2 were not tested.

The `*-dry-run.json` receipts did not contact GitHub or create anything. Bootstrap inspection did not install software. The optional process dependency and browser existed in the authoring environment. Tool availability does not establish availability on the owner’s machine.

`synthetic-experiments.json` records only synthetic container smoke/oracle observations. See docs/22-experiment-findings.md for limitations. No personal preferences, filesystem layout, process command lines or model quality are inferred from synthetic fixtures.
