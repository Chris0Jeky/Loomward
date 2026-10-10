# Contract examples

Synthetic data only. Checked by `cargo test -p loomward-protocol` (`tests/contract.rs`): each valid
example validates against its `$def` in `../view-service.schema.json`, round-trips through the Rust
DTO to an equal JSON value, and, wrapped in a complete envelope, has its payload checked against the
type `../commands.json` names for the envelope's command or event.

| Path | Holds | `$def` |
|---|---|---|
| `commands/<command>.request.json` | the request payload | `request` in `../commands.json` |
| `commands/<command>.result.json` | the `result` of an ok response | `result` in `../commands.json` |
| `events/<event>.json` | the `data` of an event | `data` in `../commands.json` |
| `envelopes/*.json` | `{ "def", "value" }` whole envelopes | the named def |
| `dtos/*.json` | `{ "def", "value" }` for a def that is no command payload (`DisclosureSummary`) | the named def |
| `invalid/*.json` | `{ "def", "reason", "value" }` that must be rejected | the named def |

An invalid case with `"complete": true` also checks the payload against the type `commands.json`
names for the envelope's command or event. `"rust_only": true` marks a rule JSON Schema cannot express
(the `u64` range, `expected_state_rev` on a command without a precondition): the schema accepts it and
the Rust runtime check must reject it.

A new command or event needs its examples here in the same change.
