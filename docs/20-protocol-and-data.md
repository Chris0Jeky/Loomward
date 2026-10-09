# Data contracts and versioning

## Prototype contracts
`schemas/inventory-v1.json`, `feedback-v1.json` and `teacher-decision-v1.json` document the interchange surface. Runtime validators are authoritative in this reference; the JSON schemas are documentation/validation aids and do not grant permissions.

Inventory contains an explicit mode/root, file observations, aggregate logical size, known per-entry allocations, unknown allocation count and coverage/error/limit state. Unknown allocation is null, not inferred from logical size. IDs are snapshot observation keys. Python and Rust metadata records are not interchangeable for content authorization: the Python duplicate reader requires its own portable-stat evidence; Rust currently emits no content-reader contract.

Feedback export contains mode and complete scope-specific events, with event ID, item ID, revision, source, label, bounded metadata, optional scope/time and retraction. It excludes synthetic seed events from user feedback export. A demo feedback export is still synthetic and must be explicitly allowed for offline training. A human retraction removes the selected item's training influence on refit; it does not erase historical event/audit storage automatically.

Teacher output identifies the exact item and a taxonomy label or abstention. Evidence fields reference only accepted input keys. Any unknown field, command, arbitrary path or item mismatch is rejected. An accepted teacher record is weak advice, not automatically a persisted human correction.

Tier scenarios specify 1..32 volumes, disjoint groups, source goal and transfer budget. Each group supplies independent source/destination/transfer byte quantities and explicit activity/pin/protection state. Heat/history may be unknown, which makes the group ineligible. The UI's synthetic scenario is not filled in from a live disk scan.

## Version-two direction
Use decimal strings for exact 64/128-bit IDs, byte counters above the UI safe-integer range and nanosecond timestamps; use typed binary internal APIs where appropriate. Keep display strings separate from filesystem path objects. Add named volumes/capabilities, index generations, per-root grants, stable logical-item identity, evidence references, taxonomy/feature/model versions, operation manifests and backup coverage.

Validate envelope size, field count, nested depth, collection length and value bounds. Unknown fields are rejected on authority-bearing operations. Forward-compatible read-only snapshots may retain extra metadata without interpreting it as a capability. Protocol-version mismatch produces a specific error and no effects.

## Future database schema
`catalogue-v2.sql` is a proposed separate native catalogue, not a migration against the existing feedback database. It includes scope, volume, object, location, group, feature, observation, model, proposal, approval, operation and protection tables. A future migration must use an application ID, transactional version changes, backup/rollback strategy, WAL handling and compatibility tests. Loading a schema file does not implement those lifecycle guarantees.

The database is not trusted authority for current filesystem identity. An executor validates current handles and grants. An immutable plan manifest should be serialised canonically and content-addressed; cryptographic hashes detect differences, not the truth of the observations that were hashed.
