-- Rules v4 (v3 plus Windows leftovers; v3 is v2 plus ComfyUI, A1111 and Steam): safe rows are inlined into indexed lookups, never materialised across the catalogue.
-- The top-down walk stops at stores, dependency/git directories and v1 project outputs.
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
      WHEN d.name IN ('node_modules','.git') THEN 'pruned'
      -- A recorded Cargo manifest vetoes shared targets even if it is shielded or stale.
      WHEN d.name='target'
        AND EXISTS(SELECT 1 FROM file f WHERE f.dir_id=d.parent_id AND f.name='Cargo.toml') THEN 'pruned'
      -- Exact v1 output/manifest pairs, with its safe-file and same-run observation rules.
      WHEN d.name IN ('.venv','venv','__pycache__')
        AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.parent_id
          AND f.name IN ('pyproject.toml','requirements.txt')) THEN 'pruned'
      WHEN d.name IN ('bin','obj')
        AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.parent_id
          AND (f.name GLOB '?*.csproj' OR f.name GLOB '?*.sln')) THEN 'pruned'
      WHEN d.name IN ('build','.gradle')
        AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.parent_id
          AND f.name IN ('build.gradle','build.gradle.kts')) THEN 'pruned'
      WHEN d.name='packages' AND p.name='.nuget' THEN 'nuget'
      WHEN d.name='models' AND p.name='.lmstudio' THEN 'lmstudio'
      -- ComfyUI: each child of a models directory whose parent holds comfy and custom_nodes.
      WHEN p.name='models'
        AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=p.parent_id AND c.name='comfy')
        AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=p.parent_id AND c.name='custom_nodes') THEN 'comfyui'
      -- Steam: each game folder under steamapps/common, where steamapps records an app manifest.
      WHEN p.name='common' AND g.name='steamapps'
        AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=g.id AND f.name GLOB 'appmanifest_*.acf') THEN 'steam'
      -- Windows leftovers (rules v4): parent-chain names only, wherever AppData sits under the grant.
      WHEN d.name COLLATE NOCASE IN ('Temp','CrashDumps','D3DSCache')
        AND p.name COLLATE NOCASE='Local' AND g.name COLLATE NOCASE='AppData' THEN
        CASE lower(d.name) WHEN 'temp' THEN 'temp' WHEN 'crashdumps' THEN 'crash-dumps'
          ELSE 'shader-cache' END
      WHEN ((d.name COLLATE NOCASE IN ('DXCache','GLCache') AND p.name COLLATE NOCASE='NVIDIA')
          OR (d.name COLLATE NOCASE='DxCache' AND p.name COLLATE NOCASE='AMD'))
        AND g.name COLLATE NOCASE='Local'
        AND EXISTS(SELECT 1 FROM dir h WHERE h.id=g.parent_id AND h.name COLLATE NOCASE='AppData')
        THEN 'shader-cache'
      WHEN d.name COLLATE NOCASE='Cache' AND p.name COLLATE NOCASE='DeliveryOptimization'
        AND g.name COLLATE NOCASE='Windows'
        AND EXISTS(SELECT 1 FROM dir h JOIN dir i ON i.id=h.parent_id JOIN dir j ON j.id=i.parent_id
          JOIN dir k ON k.id=j.parent_id
          WHERE h.id=g.parent_id AND h.name COLLATE NOCASE='Microsoft' AND i.name COLLATE NOCASE='Local'
            AND j.name COLLATE NOCASE='AppData' AND k.name COLLATE NOCASE='NetworkService')
        THEN 'delivery-optimization'
      -- Root-level advice is queried first; keep shielding its subtrees here, v4 stores included.
      WHEN p.id=?1 AND (d.name COLLATE NOCASE IN ('Windows.old','$WINDOWS.~BT','$WINDOWS.~WS')
        OR (d.name COLLATE NOCASE IN ('AMD','NVIDIA')
          AND d.listing_state IN ('complete','incomplete')
          AND coalesce(d.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
          -- Same driver-payload predicate as machine-stores-root.sql.
          AND (EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name COLLATE NOCASE='Setup.exe')
            OR EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id
              AND (c.name COLLATE NOCASE IN ('Packages','Display.Driver','DisplayDriver')
                OR EXISTS(SELECT 1 FROM files f WHERE f.dir_id=c.id AND f.name COLLATE NOCASE='Setup.exe'))))))
        THEN 'pruned'
      WHEN p.parent_id=?1 AND (
        (p.name COLLATE NOCASE='Windows'
          AND d.name COLLATE NOCASE IN ('Temp','Minidump','LiveKernelReports','Installer'))
        OR (p.name COLLATE NOCASE='ProgramData' AND d.name COLLATE NOCASE='Package Cache')) THEN 'pruned'
      WHEN d.name COLLATE NOCASE IN ('ReportArchive','ReportQueue') AND p.name COLLATE NOCASE='WER'
        AND g.name COLLATE NOCASE='Windows'
        AND EXISTS(SELECT 1 FROM dir h JOIN dir i ON i.id=h.parent_id
          WHERE h.id=g.parent_id AND h.name COLLATE NOCASE='Microsoft'
            AND i.name COLLATE NOCASE='ProgramData' AND i.parent_id=?1) THEN 'pruned'
      WHEN d.name='$Recycle.Bin' AND p.id=?1 THEN 'pruned'
      WHEN d.name IN ('WinSxS','SoftwareDistribution') AND p.name='Windows'
        AND p.parent_id=?1 THEN 'pruned'
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
        WHEN d.name='models'
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name='Stable-diffusion')
          AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.parent_id AND f.name='webui.py') THEN 'a1111'
        WHEN (d.name IN ('store','.pnpm-store') OR p.name='pnpm')
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id
          AND c.name GLOB 'v[0-9]*' AND substr(c.name,2) NOT GLOB '*[^0-9]*'
          AND c.listing_state IN ('complete','incomplete')
          AND coalesce(c.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
          AND EXISTS(SELECT 1 FROM safe g WHERE g.parent_id=c.id AND g.name='files')) THEN 'pnpm'
        WHEN p.listing_state='complete' AND coalesce(p.listing_outcome,'complete')='complete'
          AND NOT EXISTS(SELECT 1 FROM file f WHERE f.dir_id=d.parent_id AND f.name='Cargo.toml')
          AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='CACHEDIR.TAG')
          AND EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='.rustc_info.json')
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name IN ('debug','release')) THEN 'cargo-target'
        WHEN EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='CACHEDIR.TAG')
          AND EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id AND c.name GLOB 'archive-v*') THEN 'uv'
        WHEN EXISTS(SELECT 1 FROM safe c WHERE c.parent_id=d.id
          AND (c.name GLOB 'models--*' OR c.name GLOB 'datasets--*')
          AND c.listing_state IN ('complete','incomplete')
          AND coalesce(c.listing_outcome,'complete') NOT IN ('stale','excluded','denied','unscanned')
          AND EXISTS(SELECT 1 FROM safe g WHERE g.parent_id=c.id AND g.name='blobs')
          AND EXISTS(SELECT 1 FROM safe g WHERE g.parent_id=c.id AND g.name='snapshots')) THEN 'huggingface'
        WHEN EXISTS(SELECT 1 FROM files f WHERE f.dir_id=d.id AND f.name='.git') THEN 'worktree'
      END
    END rule
  FROM safe d LEFT JOIN dir p ON p.id=d.parent_id LEFT JOIN dir g ON g.id=p.parent_id
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
  FROM tree t JOIN dir d ON d.id=t.id WHERE t.rule IS NOT NULL AND t.rule!='pruned'
  UNION ALL
  SELECT f.id,1,
    CASE WHEN f.name='ext4.vhdx' THEN 'wsl'
      ELSE 'docker' END,
    CASE WHEN NOT ?4 THEN f.logical END, f.allocated IS NULL
  FROM tree t JOIN files f ON f.dir_id=t.id
  WHERE t.rule IS NULL AND (f.name='ext4.vhdx'
    OR f.name COLLATE NOCASE IN ('docker_data.vhdx','Docker.vhdx'))
)
SELECT id,kind,rule,max(logical IS NULL OR allocation_unknown!=0) OVER ()
FROM candidates ORDER BY logical DESC,id,kind LIMIT ?3
