# Contract examples

Synthetic data only. Checked by `cargo test -p loomward-protocol` (`tests/contract.rs`): each valid
example validates against its `$def` in `../view-service.schema.json` and round-trips through the Rust
DTO to an equal JSON value; each invalid example fails both.

| Path | Holds | `$def` |
|---|---|---|
| `commands/<command>.request.json` | the request payload | `request` in `../commands.json` |
| `commands/<command>.result.json` | the `result` of an ok response | `result` in `../commands.json` |
| `events/<event>.json` | the `data` of an event | `data` in `../commands.json` |
| `envelopes/*.json` | `{ "def", "value" }` whole envelopes | the named def |
| `invalid/*.json` | `{ "def", "reason", "value" }` that must be rejected | the named def |

A new command or event needs its examples here in the same change.
