# Synthetic inference broker: design for owner review

Date: 2026-10-10. Related: LW-111 / [issue 130](https://github.com/Chris0Jeky/Loomward/issues/130).
Status: architecture direction approved in HUMAN_TODO q-8; this concrete design awaits review.
No broker implementation or connected-provider qualification is claimed.

## Goal and success criteria

Provide Loomward's optional teacher with bounded inference inputs and structured advice,
without exposing a general agent executor. Preserve editable, testable interfaces and honest
unavailable states. First prove the contract offline on generated data, then qualify one
supported provider transport. Personal data remains disabled.

The owner chose broker-first over AppContainer registration. No profile registration, ACL,
firewall, credential copying or paid fallback is included. Read-only diagnosis of Kraspyon
is a separate approved task, currently blocked by offline/unavailable task access.

## Current evidence

The native teacher preview/run code in crates/loomward-engine/src/teacher/mod.rs is a stub.
Personal runs fail with confinement_not_enforced and synthetic runs report unavailable.
The Python teacher is a separate loopback reference, not native integration.
PR174 never launched contained Codex, so it proves neither authentication nor confinement.
Issue176 tracks uncorrected spike receipt races; do not reuse that spike as qualification.

docs/41 section9.2 and contracts/v3/semantics.md section11 remain binding for personal
disclosure. This synthetic design does not silently waive their OS-enforcement requirements.
A future personal-mode proposal must explicitly reconcile those requirements and q-5.

## Selected architecture

1. A pure Rust contract module validates and serializes immutable requests and replies.
   Start under experiments/teacher-confinement/broker/ to respect the L15b lane.
   It is a lab artifact, not a new public command or production crate in this first slice.
2. A fixed offline provider exercises the contract using committed synthetic fixtures.
   It is explicitly labelled mock; it has no network or process execution dependency.
3. A later inference-only provider adapter uses a documented transport, without invoking
   Codex CLI, shell, MCP, plugins or child agents. The account/model and authentication route
   are separately qualified. Unknown eligibility returns unavailable, never fallback.
4. Native engine integration follows a dedicated review and preserves the existing preview,
   grant, durable request state and personal refusal. No current public DTO is changed here.

A fixed offline registry maps fixture IDs to generated metadata. The initial harness accepts
fixture IDs, not caller-supplied names, paths, prompts or a self-declared synthetic boolean.
Later registered-lab support must verify existing lab provenance and identity rather than
trusting a string flag.

## Request and response boundary

- Versioned closed schema; reject duplicate and unknown fields, malformed UTF-8, trailing
  JSON, non-finite values and excessive nesting.
- Maximum 25 unique request-local item handles. Reuse the existing teacher metadata field
  bounds and allowed-label semantics from the learning contract. No arbitrary file contents.
- Maximum serialized request 64 KiB, response 64 KiB, JSON nesting16; enforce bytes before
  allocating/parser work beyond the limit. Fixture count and request limits are independent.
- The trusted builder supplies instructions, output schema and model configuration.
  Callers cannot provide endpoints, credentials, headers, tool definitions, file IDs,
  conversation IDs, arbitrary provider options or callback URLs.
- Bind payload digest to complete canonical serialization: metadata, labels, instructions,
  output schema, schema version and provider profile. Test that any relevant change alters it.
- Reply identity must match the request exactly; reject missing, extra or repeated items,
  unknown labels and executable/tool payloads. Abstention is a valid explicit outcome.
- Returned advice remains source=teacher, weight=0.2, requires_review=true,
  autonomy_allowed=false. A score is not an operational grant.

Hostile names and tool-shaped responses are offline fixtures only. Prompt wording is defence
in depth, not the enforcement boundary; no response is ever interpreted as code or a tool call.

## Execution and recovery

The offline stage runs in-process with bounded inputs and deterministic output; no daemon,
listener, authentication or new OS privileges. One explicit invocation, no automatic discovery
of files or environment credentials.

For a future connected adapter, one request at a time, bounded response reading and a hard
180-second total deadline. No automatic retry after an attempted dispatch: timeout or crash
can mean the provider received the request. Record unknown/interrupted, not success or
safe-to-retry. Before connected integration, the durable transaction must consume the grant
and record the attempt before egress, as semantics11 requires. Repeated calls return the same
job/receipt and cannot repeat dispatch.

A transport adapter may not follow redirects or accept model-provided network destinations.
Endpoint selection and lack of tool dispatch reduce capabilities but do not prove OS-level
egress or filesystem isolation. Those remain separate, explicitly unverified properties.

## Provider qualification gate

No live provider is selected or connected by this spec. Evaluate the supported ChatGPT
application sign-in route against actual account/model eligibility, with official documentation
at implementation time. Do not copy Codex authentication files. New persistent authentication
requires an explicit user-approved flow. If unavailable, stop and present alternatives rather
than switching billing or invoking an agent CLI.

Credentials, raw prompts and provider response text never enter public receipts. Qualification
requires a synthetic request, strict valid reply, capability inspection and negative tests.
A provider that cannot deliver an inference-only interface is not accepted.

## Verification

First slice:
- Test-first closed-schema, byte/depth/count bounds and duplicate rejection.
- Fixture identity/provenance refusal, including fabricated synthetic claims.
- Complete digest binding and deterministic serialization.
- Valid advice, abstention, unknown labels, repeated/missing items, tool-shaped response refusal.
- Assert the offline harness makes no network calls or child-process launches.
- Full Rust formatting, tests and clippy for its actual manifest, plus repository applicable
  gates before integration. Report Windows and Linux separately; absent tools are unverified.

Later integration additionally needs preview expiry, revocation, consumed-grant replay,
dispatch crash/timeout, profile mismatch and provider failure tests. Check raw diagnostic
output cannot leak fixture input or credentials. An offline mock pass is never reported as
provider, Windows confinement or personal-data qualification.

## Deferred scope and review checkpoint

No personal metadata, new production API, automatic scans, real training updates, new
authentication, paid API fallback or security-setting change in the first slice.
AppContainer remains a possible future experiment, not an approved side task.

After review of this design, write the implementation plan for the offline contract and
harness, with direct execution in this conversation. Provider connection and native
integration remain separately evidenced milestones.
