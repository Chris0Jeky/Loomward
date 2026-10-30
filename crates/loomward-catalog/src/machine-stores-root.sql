-- Root-level rules finish before the recursive walk, using direct children only.
WITH
safe AS NOT MATERIALIZED (
  SELECT * FROM dir WHERE flags & ?2=0 AND attrs & 1024=0
    AND listing_state NOT IN ('excluded','denied','absent_pending')
),
root AS NOT MATERIALIZED (
  SELECT * FROM safe WHERE id=?1 AND name NOT IN ('node_modules','.git')
    AND listing_state IN ('complete','incomplete')
    AND coalesce(listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
),
directories AS (
  SELECT d.*, 'recycle-bin' AS rule
  FROM root r JOIN safe d ON d.parent_id=r.id WHERE d.name='$Recycle.Bin'
  UNION ALL
  SELECT d.*, 'system' AS rule
  FROM root r JOIN safe w ON w.parent_id=r.id JOIN safe d ON d.parent_id=w.id
  WHERE w.name='Windows' AND d.name IN ('WinSxS','SoftwareDistribution')
    AND w.listing_state IN ('complete','incomplete')
    AND coalesce(w.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
),
candidates(id,kind,rule,logical,allocation_unknown) AS (
  SELECT id,0,rule,
    CASE WHEN sub_complete=1 AND listing_state='complete'
      AND coalesce(listing_outcome,'complete')='complete'
      AND agg_valid_rev>=dirty_rev AND NOT ?4 THEN sub_logical END,
    sub_alloc_unknown
  FROM directories
  UNION ALL
  SELECT f.id,1,'system',CASE WHEN NOT ?4 THEN f.logical END,f.allocated IS NULL
  FROM root r JOIN file f ON f.dir_id=r.id
  WHERE f.name IN ('hiberfil.sys','pagefile.sys','swapfile.sys')
    AND f.flags & ?2=0 AND f.attrs & 1024=0
    AND (r.listing_state='complete' OR f.seen_run=r.seen_run)
)
SELECT id,kind,rule,max(logical IS NULL OR allocation_unknown!=0) OVER ()
FROM candidates ORDER BY logical DESC,id,kind LIMIT ?3
