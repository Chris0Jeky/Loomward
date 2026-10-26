-- One scoped, shield-respecting walk. No aggregate repair or catalogue writes.
WITH RECURSIVE tree(id,depth) AS (
  SELECT id,0 FROM dir WHERE id=?1 AND flags & ?2=0 AND attrs & 1024=0
    AND listing_state NOT IN ('excluded','denied','absent_pending')
  UNION ALL
  SELECT d.id,t.depth+1 FROM tree t JOIN dir p ON p.id=t.id
    JOIN dir d ON d.parent_id=t.id AND d.root_id=p.root_id
  WHERE d.flags & ?2=0 AND d.attrs & 1024=0
    AND d.listing_state NOT IN ('excluded','denied','absent_pending')
    AND p.listing_state IN ('complete','incomplete')
    AND coalesce(p.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned','cancelled')
)
SELECT 0 kind,d.id,d.parent_id,d.name,t.depth,
  d.listing_state='complete' AND coalesce(d.listing_outcome,'complete')='complete'
    AND d.sub_complete=1 AND d.agg_valid_rev>=d.dirty_rev known,
  NULL logical,NULL allocated,NULL modified_ft,NULL link_count,NULL ext,NULL family
FROM tree t JOIN dir d ON d.id=t.id
UNION ALL
SELECT 1,f.dir_id,NULL,f.name,t.depth+1,1,f.logical,f.allocated,f.modified_ft,f.link_count,e.ext,e.family
FROM tree t JOIN dir d ON d.id=t.id JOIN file f ON f.dir_id=t.id LEFT JOIN ext e ON e.id=f.ext_id
WHERE f.flags & ?2=0 AND f.attrs & 1024=0
  AND d.listing_state IN ('complete','incomplete')
  AND coalesce(d.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned','cancelled')
  AND (d.listing_state='complete' OR f.seen_run=d.seen_run)
