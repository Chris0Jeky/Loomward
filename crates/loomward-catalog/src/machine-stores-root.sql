-- Root-level rules finish before the recursive walk, using direct children only.
-- Rules v4 Windows names compare ASCII case-insensitively; the v3 names above them stay exact.
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
listed AS NOT MATERIALIZED (
  SELECT * FROM safe WHERE listing_state IN ('complete','incomplete')
    AND coalesce(listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
),
files AS NOT MATERIALIZED (
  SELECT f.* FROM file f JOIN listed d ON d.id=f.dir_id
  WHERE f.flags & ?2=0 AND f.attrs & 1024=0
    AND (d.listing_state='complete' OR f.seen_run=d.seen_run)
),
-- One pass over the root's children: the arms below pick from these few rows.
top AS MATERIALIZED (
  SELECT d.* FROM root r JOIN safe d ON d.parent_id=r.id
  WHERE d.name COLLATE NOCASE IN ('$Recycle.Bin','Windows','Windows.old','$WINDOWS.~BT','$WINDOWS.~WS',
    'ProgramData','AMD','NVIDIA')
),
windows AS NOT MATERIALIZED (
  SELECT t.* FROM top t WHERE t.name COLLATE NOCASE='Windows'
    AND EXISTS(SELECT 1 FROM listed l WHERE l.id=t.id)
),
programdata AS NOT MATERIALIZED (
  SELECT t.* FROM top t WHERE t.name COLLATE NOCASE='ProgramData'
    AND EXISTS(SELECT 1 FROM listed l WHERE l.id=t.id)
),
directories AS (
  SELECT d.*, 'recycle-bin' AS rule
  FROM top d WHERE d.name='$Recycle.Bin'
  UNION ALL
  SELECT d.*, 'system' AS rule
  FROM windows w CROSS JOIN safe d ON d.parent_id=w.id
  WHERE w.name='Windows' AND d.name IN ('WinSxS','SoftwareDistribution')
  UNION ALL
  -- Previous Windows installation(s): directly under the granted root.
  SELECT d.*, 'windows-old' AS rule
  FROM top d WHERE d.name COLLATE NOCASE IN ('Windows.old','$WINDOWS.~BT','$WINDOWS.~WS')
  UNION ALL
  SELECT d.*, CASE lower(d.name) WHEN 'temp' THEN 'temp' WHEN 'installer' THEN 'installer-cache'
    ELSE 'memory-dump' END AS rule
  FROM windows w CROSS JOIN safe d ON d.parent_id=w.id
  WHERE d.name COLLATE NOCASE IN ('Minidump','LiveKernelReports','Temp','Installer')
  UNION ALL
  SELECT d.*, 'delivery-optimization' AS rule
  FROM windows w CROSS JOIN listed s ON s.parent_id=w.id CROSS JOIN safe d ON d.parent_id=s.id
  WHERE s.name COLLATE NOCASE='SoftwareDistribution' AND d.name COLLATE NOCASE='DeliveryOptimization'
  UNION ALL
  SELECT d.*, 'installer-cache' AS rule
  FROM programdata p CROSS JOIN safe d ON d.parent_id=p.id WHERE d.name COLLATE NOCASE='Package Cache'
  UNION ALL
  SELECT d.*, 'error-reports' AS rule
  FROM programdata p CROSS JOIN listed m ON m.parent_id=p.id CROSS JOIN listed i ON i.parent_id=m.id
    CROSS JOIN listed e ON e.parent_id=i.id CROSS JOIN safe d ON d.parent_id=e.id
  WHERE m.name COLLATE NOCASE='Microsoft' AND i.name COLLATE NOCASE='Windows'
    AND e.name COLLATE NOCASE='WER' AND d.name COLLATE NOCASE IN ('ReportArchive','ReportQueue')
  UNION ALL
  -- Driver installer extracts: AMD or NVIDIA directly under the root holding an installer payload
  -- (keep this predicate in step with the walk's shield in machine-stores.sql).
  SELECT d.*, 'driver-extract' AS rule
  FROM top d
  WHERE d.name COLLATE NOCASE IN ('AMD','NVIDIA') AND EXISTS(SELECT 1 FROM listed l WHERE l.id=d.id)
    AND (EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name COLLATE NOCASE='Setup.exe')
      OR EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id
        AND (c.name COLLATE NOCASE IN ('Packages','Display.Driver','DisplayDriver')
          OR EXISTS(SELECT 1 FROM files f WHERE f.dir_id=c.id AND f.name COLLATE NOCASE='Setup.exe'))))
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
  UNION ALL
  SELECT f.id,1,'memory-dump',CASE WHEN NOT ?4 THEN f.logical END,f.allocated IS NULL
  FROM windows w CROSS JOIN file f ON f.dir_id=w.id
  WHERE f.name COLLATE NOCASE='MEMORY.DMP'
    AND f.flags & ?2=0 AND f.attrs & 1024=0
    AND (w.listing_state='complete' OR f.seen_run=w.seen_run)
)
SELECT id,kind,rule,max(logical IS NULL OR allocation_unknown!=0) OVER ()
FROM candidates ORDER BY logical DESC,id,kind LIMIT ?3
