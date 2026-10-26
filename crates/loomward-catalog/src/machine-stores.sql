-- Rules v2: safe rows are inlined into indexed lookups, never materialised across the catalogue.
-- The top-down walk stops at a store; only its signature is read, never its interior candidates.
WITH RECURSIVE
safe AS NOT MATERIALIZED (
  SELECT * FROM dir WHERE flags & ?2=0 AND attrs & 1024=0
    AND listing_state NOT IN ('excluded','denied','absent_pending')
),
files AS NOT MATERIALIZED (
  SELECT f.* FROM file f JOIN safe d ON d.id=f.dir_id
  WHERE f.flags & ?2=0 AND f.attrs & 1024=0
    AND d.listing_state IN ('complete','incomplete')
    AND coalesce(d.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
    AND (d.listing_state='complete' OR f.seen_run=d.seen_run)
),
rules AS NOT MATERIALIZED (
  SELECT d.id,
    CASE
      WHEN d.name='packages' AND p.name='.nuget' THEN 'nuget'
      WHEN d.name='models' AND p.name='.lmstudio' THEN 'lmstudio'
      WHEN d.name='$Recycle.Bin' AND p.id=?1 THEN 'recycle-bin'
      WHEN d.name IN ('WinSxS','SoftwareDistribution') AND p.name='Windows'
        AND p.parent_id=?1 THEN 'system'
      WHEN d.listing_state IN ('complete','incomplete')
        AND coalesce(d.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
      THEN CASE
        WHEN d.name='_cacache'
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='content-v2')
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='index-v5') THEN 'npm'
        WHEN d.name='pip'
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name IN ('http-v2','http','wheels')) THEN 'pip'
        WHEN d.name='caches' AND p.name='.gradle'
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='modules-2') THEN 'gradle'
        WHEN p.name='.cargo' AND (
          (d.name='registry' AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='cache')) OR
          (d.name='git' AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='db'))
        ) THEN 'cargo-cache'
        WHEN d.name='models'
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='blobs')
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='manifests') THEN 'ollama'
        WHEN EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name IN ('debug','release'))
          AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='CACHEDIR.TAG')
          AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='.rustc_info.json')
          -- Any recorded manifest vetoes a shared target, even if it is shielded or stale.
          AND NOT EXISTS(SELECT 1 FROM file f WHERE f.dir_id=d.parent_id AND f.name='Cargo.toml') THEN 'cargo-target'
        WHEN EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name GLOB 'archive-v*')
          AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='CACHEDIR.TAG') THEN 'uv'
        WHEN EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id
          AND c.name GLOB 'v[0-9]*' AND substr(c.name,2) NOT GLOB '*[^0-9]*'
          AND c.listing_state IN ('complete','incomplete')
          AND coalesce(c.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
          AND EXISTS(SELECT 1 FROM safe g WHERE g.parent_id=c.id AND g.name='files')) THEN 'pnpm'
        WHEN EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id
          AND (c.name GLOB 'models--*' OR c.name GLOB 'datasets--*')
          AND c.listing_state IN ('complete','incomplete')
          AND coalesce(c.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
          AND EXISTS(SELECT 1 FROM safe g WHERE g.parent_id=c.id AND g.name='blobs')
          AND EXISTS(SELECT 1 FROM safe g WHERE g.parent_id=c.id AND g.name='snapshots')) THEN 'huggingface'
        WHEN EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='.git') THEN 'worktree'
      END
    END rule
  FROM safe d LEFT JOIN dir p ON p.id=d.parent_id
),
tree(id,rule) AS (
  SELECT id,rule FROM rules WHERE id=?1
  UNION ALL
  SELECT d.id,r.rule FROM tree t JOIN dir p ON p.id=t.id
    JOIN safe d ON d.parent_id=t.id JOIN rules r ON r.id=d.id
  WHERE t.rule IS NULL AND p.listing_state IN ('complete','incomplete')
    AND coalesce(p.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
),
candidates(id,kind,rule,logical,allocation_unknown) AS (
  SELECT d.id,0,t.rule,
    CASE WHEN d.sub_complete=1 AND d.listing_state='complete'
      AND coalesce(d.listing_outcome,'complete')='complete'
      AND d.agg_valid_rev>=d.dirty_rev AND NOT ?4 THEN d.sub_logical END,
    d.sub_alloc_unknown
  FROM tree t JOIN dir d ON d.id=t.id WHERE t.rule IS NOT NULL
  UNION ALL
  SELECT f.id,1,
    CASE WHEN f.name='ext4.vhdx' THEN 'wsl'
      WHEN f.name COLLATE NOCASE IN ('docker_data.vhdx','Docker.vhdx') THEN 'docker'
      ELSE 'system' END,
    CASE WHEN NOT ?4 THEN f.logical END, f.allocated IS NULL
  FROM tree t JOIN files f ON f.dir_id=t.id
  WHERE t.rule IS NULL AND (f.name='ext4.vhdx'
    OR f.name COLLATE NOCASE IN ('docker_data.vhdx','Docker.vhdx')
    OR (f.dir_id=?1 AND f.name IN ('hiberfil.sys','pagefile.sys','swapfile.sys')))
)
SELECT id,kind,rule,max(logical IS NULL OR allocation_unknown!=0) OVER ()
FROM candidates ORDER BY logical DESC,id,kind LIMIT ?3
