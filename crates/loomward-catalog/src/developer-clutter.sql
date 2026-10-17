-- Developer-clutter detector (rules version 1), shaped for #515 (the v1 shape is kept as the
-- differential oracle in tests/fixtures/developer-clutter-v1.sql). One top-down walk of the safe
-- tree that stops at the first matching output directory, so dependency and build interiors are
-- never visited. A directory's files are read at most once, and only when it has a child with an
-- output name: its `prof` holds the lowest anchor file id per rule family (JSON keys c cargo,
-- n npm, y python, d dotnet, g gradle) and each output-named child takes its anchor from that.
-- Cost: (directories outside outputs) + (files in directories with an output-named child);
-- never files x child directories x rules. Without a (dir_id, name) index the second term is a
-- scan of those directories' files. Names are exact and case-sensitive. Rules, anchor beside
-- output: Cargo.toml target; package.json node_modules; pyproject.toml|requirements.txt
-- .venv|venv|__pycache__; *.csproj|*.sln bin|obj; build.gradle[.kts] build|.gradle.
WITH RECURSIVE
tree(id, anchor_id, prof) AS (
  SELECT d.id, NULL,
    CASE WHEN EXISTS(SELECT 1 FROM dir c WHERE c.parent_id=d.id
        AND c.name IN ('target','node_modules','.venv','venv','__pycache__','bin','obj','build','.gradle')) THEN
      (SELECT json_object('c',min(CASE WHEN f.name='Cargo.toml' THEN f.id END),
          'n',min(CASE WHEN f.name='package.json' THEN f.id END),
          'y',min(CASE WHEN f.name IN ('pyproject.toml','requirements.txt') THEN f.id END),
          'd',min(CASE WHEN f.name GLOB '?*.csproj' OR f.name GLOB '?*.sln' THEN f.id END),
          'g',min(CASE WHEN f.name IN ('build.gradle','build.gradle.kts') THEN f.id END))
        FROM file f WHERE f.dir_id=d.id
          AND (f.name IN ('Cargo.toml','package.json','pyproject.toml','requirements.txt','build.gradle','build.gradle.kts')
            OR f.name GLOB '?*.[cs][sl][pn]*')
          AND f.flags & ?2=0 AND f.attrs & 1024=0
          AND (d.listing_state='complete' OR f.seen_run=d.seen_run)) END
  FROM dir d WHERE d.id=?1 AND d.flags & ?2=0 AND d.attrs & 1024=0
    AND d.listing_state NOT IN ('excluded','denied','absent_pending')
  UNION ALL
  SELECT d.id,
    CASE d.name WHEN 'target' THEN json_extract(t.prof,'$.c')
      WHEN 'node_modules' THEN json_extract(t.prof,'$.n')
      WHEN '.venv' THEN json_extract(t.prof,'$.y') WHEN 'venv' THEN json_extract(t.prof,'$.y')
      WHEN '__pycache__' THEN json_extract(t.prof,'$.y')
      WHEN 'bin' THEN json_extract(t.prof,'$.d') WHEN 'obj' THEN json_extract(t.prof,'$.d')
      WHEN 'build' THEN json_extract(t.prof,'$.g') WHEN '.gradle' THEN json_extract(t.prof,'$.g') END,
    CASE WHEN EXISTS(SELECT 1 FROM dir c WHERE c.parent_id=d.id
        AND c.name IN ('target','node_modules','.venv','venv','__pycache__','bin','obj','build','.gradle')) THEN
      (SELECT json_object('c',min(CASE WHEN f.name='Cargo.toml' THEN f.id END),
          'n',min(CASE WHEN f.name='package.json' THEN f.id END),
          'y',min(CASE WHEN f.name IN ('pyproject.toml','requirements.txt') THEN f.id END),
          'd',min(CASE WHEN f.name GLOB '?*.csproj' OR f.name GLOB '?*.sln' THEN f.id END),
          'g',min(CASE WHEN f.name IN ('build.gradle','build.gradle.kts') THEN f.id END))
        FROM file f WHERE f.dir_id=d.id
          AND (f.name IN ('Cargo.toml','package.json','pyproject.toml','requirements.txt','build.gradle','build.gradle.kts')
            OR f.name GLOB '?*.[cs][sl][pn]*')
          AND f.flags & ?2=0 AND f.attrs & 1024=0
          AND (d.listing_state='complete' OR f.seen_run=d.seen_run)) END
  FROM tree t JOIN dir p ON p.id=t.id JOIN dir d ON d.parent_id=t.id
  WHERE t.anchor_id IS NULL
    AND d.flags & ?2=0 AND d.attrs & 1024=0
    AND d.listing_state NOT IN ('excluded','denied','absent_pending')
    AND p.listing_state IN ('complete','incomplete')
    AND coalesce(p.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
)
SELECT d.id, t.anchor_id, f.name,
  CASE d.name WHEN 'target' THEN 'cargo' WHEN 'node_modules' THEN 'npm'
    WHEN 'bin' THEN 'dotnet' WHEN 'obj' THEN 'dotnet'
    WHEN 'build' THEN 'gradle' WHEN '.gradle' THEN 'gradle' ELSE 'python' END
FROM tree t JOIN dir d ON d.id=t.id JOIN file f ON f.id=t.anchor_id
ORDER BY CASE WHEN d.sub_complete=1 AND d.listing_state='complete'
  AND coalesce(d.listing_outcome,'complete')='complete'
  AND d.agg_valid_rev>=d.dirty_rev AND NOT ?4 THEN d.sub_logical END DESC, d.id
LIMIT ?3
