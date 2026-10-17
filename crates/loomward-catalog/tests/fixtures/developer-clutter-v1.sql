WITH RECURSIVE
rules(anchor, output, provider) AS (VALUES
  ('Cargo.toml', 'target', 'cargo'),
  ('package.json', 'node_modules', 'npm'),
  ('pyproject.toml', '.venv', 'python'),
  ('pyproject.toml', 'venv', 'python'),
  ('pyproject.toml', '__pycache__', 'python'),
  ('requirements.txt', '.venv', 'python'),
  ('requirements.txt', 'venv', 'python'),
  ('requirements.txt', '__pycache__', 'python'),
  ('?*.csproj', 'bin', 'dotnet'), ('?*.csproj', 'obj', 'dotnet'),
  ('?*.sln', 'bin', 'dotnet'), ('?*.sln', 'obj', 'dotnet'),
  ('build.gradle', 'build', 'gradle'), ('build.gradle', '.gradle', 'gradle'),
  ('build.gradle.kts', 'build', 'gradle'), ('build.gradle.kts', '.gradle', 'gradle')
),
safe(id) AS (
  SELECT id FROM dir WHERE id=?1 AND flags & ?2=0 AND attrs & 1024=0
    AND listing_state NOT IN ('excluded','denied','absent_pending')
  UNION ALL
  SELECT d.id FROM dir d JOIN safe s ON d.parent_id=s.id JOIN dir p ON p.id=s.id
    WHERE d.flags & ?2=0 AND d.attrs & 1024=0
      AND d.listing_state NOT IN ('excluded','denied','absent_pending')
      AND p.listing_state IN ('complete','incomplete')
      AND coalesce(p.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
),
matched AS (
  SELECT d.id, min(f.id) anchor_id, rules.provider
  FROM safe s JOIN dir d ON d.id=s.id JOIN dir p ON p.id=d.parent_id
    JOIN file f ON f.dir_id=p.id
    JOIN rules ON f.name GLOB rules.anchor AND d.name=rules.output
  WHERE f.flags & ?2=0 AND f.attrs & 1024=0
    AND (p.listing_state='complete' OR f.seen_run=p.seen_run)
  GROUP BY d.id
),
covered(id) AS (
  SELECT id FROM matched
  UNION
  SELECT d.id FROM dir d JOIN covered c ON d.parent_id=c.id
)
SELECT d.id, m.anchor_id, f.name, m.provider
FROM matched m JOIN dir d ON d.id=m.id JOIN file f ON f.id=m.anchor_id
WHERE d.parent_id NOT IN (SELECT id FROM covered)
ORDER BY CASE WHEN d.sub_complete=1 AND d.listing_state='complete'
  AND coalesce(d.listing_outcome,'complete')='complete'
  AND d.agg_valid_rev>=d.dirty_rev AND NOT ?4 THEN d.sub_logical END DESC, d.id
LIMIT ?3
