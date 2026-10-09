# Versioned interchange contracts

Run `python scripts/export_contracts.py` from the source checkout to refresh reference-owned schema files. `tests/test_v2_contracts.py` checks schema/runtime parity and validates examples when the optional jsonschema validator is installed. The basic app requires no validator dependency.

The four input schemas and generic output envelope describe the executable snapshot tool service. Runtime validation adds semantic restrictions: byte strings must fit the reference safe-integer bound, scope cannot expand, forbidden disclosures fail, and placement must satisfy its physics. A schema match alone is not permission.

Provider, event and proposal schemas are **design-only**. There is no provider runtime, event bus or durable proposal store using them yet. Each contains a synthetic example. `INDEX.json` records these status distinctions.

MCP request files are replayable synthetic examples, not host registration files and not proof of compatibility with every client. Pipe them through the process only via the smoke helper or a configured host. The modern and legacy examples use their own lifecycle and metadata.
