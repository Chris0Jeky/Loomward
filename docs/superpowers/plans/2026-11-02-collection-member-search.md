# Direct collection-member search (#1124)

Base: GitHub main 3f5aaf2b9dd6a9c49d1c0471f4390deb6df5d10a, local verified snapshot 4db7f22494b7ba330945e7ba7df752e88da6ea55.
Preserved e69e695 contributes only app/tests/unit/collection-search.test.ts; its base is 26 commits behind this main. Reapply those preliminary tests, not the old tree.

## Contract and boundaries

Add owner-local collections.search, advertised by optional features.collection_search (absent means unsupported). Required collection_id and the existing deterministic text/type/extension/size/date predicates narrow eligible direct members before pagination. No recursive descendants, federated search fields, index migration, content reads or authority changes. Human collections only. Return SearchHit projections plus collection_id, total_members, unavailable_members, partial, budget_hit and next_cursor. Total members is not a matching count. Unavailable coverage never discloses unavailable names/paths.

Bound work to 2,000 direct memberships per page and the existing interrupt/deadline. Stable object_ref_id keyset cursor binds full query, collection, state revision and catalogue revision; service session storage fences restarts. Non-owner clients cannot invoke the operation. A revision change during hydration discards the page.

## Steps and tests

1. Preserve preliminary tests; native tests use deserialization to fail behaviorally on unknown operation without compilation errors. Coordinator publishes tests-only proof before implementation qualification.
2. Add focused native direct scope/filter/pagination, invalid scope, revision/restart/revoke, bounded work and no-descendant tests.
3. Implement bounded native service and additive contract. Protocol, central dispatch, schema, examples and generated contracts are explicit coordinator-review changes.
4. Implement mock parity and Explorer capability-gated collection choices. Freeze submitted scope across paging; stale/cancelled replies and reconnect cannot broaden it. Reuse existing inspection and state/session-fenced AddToCollection.
5. Extend dedicated native HTTP/browser collection journey; test unit functions, app check/test/build and available Python checks. Native Rust and actual native browser proof remain unverified locally until hosted CI runs them.

Owned: collections service/tests, collection mock/tests, Explorer and Collections views, dedicated collection browser journey and this plan.
Coordinator surfaces: command registry/DTO registry/session features, handlers/lib routing, contract schema/commands/examples/generated app contract, integration harness registration if needed.

## Acceptance evidence

Report exact commands/results, behavioral red separately from missing tools, old-engine absence, direct-only filtering, cursor scope/revision/session rejection, unavailable coverage, subset retention/reload, keyboard/narrow layout. No native success claim without actual native execution.
