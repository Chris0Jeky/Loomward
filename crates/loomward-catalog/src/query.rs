use crate::{
    model::{flags, timestamp},
    *,
};
use rusqlite::{params, types::Value, Connection, OpenFlags, OptionalExtension};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub struct Reader {
    pub(crate) conn: Connection,
    breakdowns: HashMap<(i64, String, Basis, usize, String), Breakdown>,
}
pub(crate) const DIR_COLUMNS:&str="d.id,0 kind,d.name,d.sub_logical logical,d.sub_allocated allocated,d.sub_alloc_unknown unknown,d.sub_files files,d.sub_dirs dirs,d.modified_ft,d.attrs,d.flags,NULL extension,NULL family,d.state,d.sub_complete complete,d.parent_id,d.grant_id,d.file_id,d.created_ft,d.changed_ft,NULL accessed_ft";
pub(crate) const FILE_COLUMNS:&str="f.id,1 kind,f.name,f.logical,coalesce(f.allocated,0) allocated,(f.allocated IS NULL) unknown,1 files,0 dirs,f.modified_ft,f.attrs,f.flags,e.ext extension,e.family,d.state,1 complete,f.dir_id parent_id,d.grant_id,f.file_id,f.created_ft,f.changed_ft,f.accessed_ft";
pub(crate) const FILE_FROM: &str =
    "file f JOIN dir d ON d.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id";
#[derive(Clone, Debug)]
pub(crate) struct Raw {
    pub key: NodeKey,
    pub name: String,
    pub totals: Totals,
    pub modified: Option<i64>,
    pub attrs: u32,
    pub flags: u32,
    pub extension: Option<String>,
    pub family: Option<String>,
    pub state: String,
    pub parent: Option<i64>,
    pub grant: i64,
    pub file_id: Option<Vec<u8>>,
    pub created: Option<i64>,
    pub changed: Option<i64>,
    pub accessed: Option<i64>,
}
pub(crate) fn raw(r: &rusqlite::Row<'_>) -> rusqlite::Result<Raw> {
    let id = r.get(0)?;
    let kind: u8 = r.get(1)?;
    Ok(Raw {
        key: if kind == 0 {
            NodeKey::Dir(id)
        } else {
            NodeKey::File(id)
        },
        name: r.get(2)?,
        totals: Totals {
            logical: r.get::<_, i64>(3)? as u64,
            allocated: r.get::<_, i64>(4)? as u64,
            allocation_unknown: r.get::<_, i64>(5)? as u64,
            files: r.get::<_, i64>(6)? as u64,
            dirs: r.get::<_, i64>(7)? as u64,
            complete: r.get(14)?,
            ..Totals::default()
        },
        modified: r.get(8)?,
        attrs: r.get(9)?,
        flags: r.get(10)?,
        extension: r.get(11)?,
        family: r.get(12)?,
        state: r.get(13)?,
        parent: r.get(15)?,
        grant: r.get(16)?,
        file_id: r.get(17)?,
        created: r.get(18)?,
        changed: r.get(19)?,
        accessed: r.get(20)?,
    })
}
impl Raw {
    pub(crate) fn coverage(&self) -> String {
        if self.flags & (1 << 6) != 0 {
            "denied".into()
        } else if self.flags & ((1 << 1) | (1 << 3) | (1 << 4) | (1 << 5)) != 0 {
            "excluded".into()
        } else if self.state == "complete" && !self.totals.complete {
            "partial".into()
        } else if matches!(self.key, NodeKey::File(_)) {
            "complete".into()
        } else {
            self.state.clone()
        }
    }
    pub(crate) fn size(&self, basis: Basis) -> u64 {
        match basis {
            Basis::Logical => self.totals.logical,
            Basis::Allocated => self.totals.allocated,
        }
    }
    pub(crate) fn entry(&self) -> EntryRow {
        const ATTRIBUTES: [(u32, &str); 15] = [
            (1, "readonly"),
            (2, "hidden"),
            (4, "system"),
            (32, "archive"),
            (512, "sparse"),
            (1024, "reparse_point"),
            (2048, "compressed"),
            (4096, "offline"),
            (16384, "encrypted"),
            (8192, "not_content_indexed"),
            (262144, "recall_on_open"),
            (4194304, "recall_on_data_access"),
            (524288, "pinned"),
            (1048576, "unpinned"),
            (256, "temporary"),
        ];
        EntryRow {
            node_id: self.key.reference(),
            kind: if matches!(self.key, NodeKey::File(_)) {
                "file"
            } else {
                "dir"
            }
            .into(),
            name: self.name.clone(),
            extension: self.extension.clone(),
            ext_family: self.family.clone(),
            logical_bytes: self.totals.logical.to_string(),
            allocated_bytes: (self.totals.allocation_unknown == 0)
                .then(|| self.totals.allocated.to_string()),
            files: Some(self.totals.files),
            dirs: Some(self.totals.dirs),
            modified_at: timestamp(self.modified),
            attributes: ATTRIBUTES
                .into_iter()
                .filter(|(b, _)| self.attrs & b != 0)
                .map(|(_, s)| s.into())
                .collect(),
            flags: flags(self.flags),
            coverage: self.coverage(),
            location_hint: None,
        }
    }
}
pub(crate) fn get(conn: &Connection, key: NodeKey) -> Result<Raw> {
    let sql=match key {NodeKey::Dir(_)=>format!("SELECT {DIR_COLUMNS} FROM dir d JOIN root_grant g ON g.id=d.grant_id WHERE d.id=?1 AND g.state='active'"),NodeKey::File(_)=>format!("SELECT {FILE_COLUMNS} FROM {FILE_FROM} JOIN root_grant g ON g.id=d.grant_id WHERE f.id=?1 AND g.state='active'"),_=>return Err(Error::Invalid("entry node expected"))};
    let id = match key {
        NodeKey::Dir(id) | NodeKey::File(id) => id,
        _ => unreachable!(),
    };
    conn.query_row(&sql, [id], raw)
        .optional()?
        .ok_or(Error::NotFound)
}
pub(crate) fn revisions(conn: &Connection, grant: Option<i64>) -> Result<Vec<(i64, i64)>> {
    let mut stmt=conn.prepare("SELECT d.grant_id,d.listing_rev FROM dir d JOIN root_grant g ON g.id=d.grant_id WHERE d.parent_id IS NULL AND g.state='active' AND (?1 IS NULL OR d.grant_id=?1) ORDER BY d.grant_id LIMIT 65")?;
    let rows = stmt
        .query_map([grant], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 64 {
        return Err(Error::Invalid("root limit"));
    }
    if grant.is_some() && rows.is_empty() {
        return Err(Error::NotFound);
    }
    Ok(rows)
}
pub(crate) fn live(conn: &Connection, grant: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM scan_run WHERE grant_id=?1 AND state='running')",
        [grant],
        |r| r.get(0),
    )?)
}
pub(crate) fn threads(conn: &Connection, grant: i64, coverage: &str, bits: u32) -> Result<Threads> {
    let (volume,key):(Option<i64>,Option<String>)=conn.query_row("SELECT g.volume_id,v.volume_key FROM root_grant g LEFT JOIN volume v ON v.id=g.volume_id WHERE g.id=?1",[grant],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let tier: Option<u8> = if let Some(key) = key {
        conn.query_row("SELECT tier FROM st.tier_declaration WHERE volume_key=?1 ORDER BY declared_at_ns DESC,id DESC LIMIT 1",[key],|r|r.get::<_,Option<u8>>(0)).optional()?.flatten()
    } else {
        None
    };
    let (state, reason) = if bits & 1 != 0 {
        ("excluded", "sensitive_name")
    } else if bits & (1 << 1) != 0 {
        ("excluded", "protected_context")
    } else if bits & (1 << 3) != 0 {
        ("excluded", "cloud_placeholder")
    } else if bits & (1 << 4) != 0 {
        ("excluded", "reparse_not_followed")
    } else {
        match coverage {
            "complete" => ("granted", "metadata_grant_active"),
            "denied" => ("denied", "access_denied"),
            "excluded" => ("excluded", "protected_context"),
            "unscanned" => ("unknown", "not_scanned"),
            _ => ("partial", "mixed_children"),
        }
    };
    Ok(Threads {
        meaning: MeaningThread {
            state: "pending".into(),
            label: None,
            share: None,
            source: None,
            collection_ids: vec![],
        },
        residency: ResidencyThread {
            volume_id: volume.map(|v| format!("vo_{v}")),
            tier,
            tier_basis: if tier.is_some() {
                "declared"
            } else {
                "unknown"
            }
            .into(),
        },
        permission: PermissionThread {
            state: state.into(),
            reason: Some(reason.into()),
        },
    })
}
pub(crate) fn direct(
    conn: &Connection,
    dir: i64,
    basis: Basis,
    limit: usize,
    include_files: bool,
    is_live: bool,
) -> Result<Vec<Raw>> {
    let dsize = if basis == Basis::Logical {
        "d.sub_logical"
    } else {
        "d.sub_allocated"
    };
    let (dorder, forder, cap) = if is_live {
        ("d.id", "f.id", limit.min(2000))
    } else {
        (dsize, "f.logical", limit)
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {DIR_COLUMNS} FROM dir d WHERE d.parent_id=?1 ORDER BY {dorder} DESC,d.id LIMIT ?2"
    ))?;
    let mut rows = stmt
        .query_map(params![dir, cap as i64], raw)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if include_files {
        let mut stmt=conn.prepare_cached(&format!("SELECT {FILE_COLUMNS} FROM {FILE_FROM} WHERE f.dir_id=?1 ORDER BY {forder} DESC,f.id LIMIT ?2"))?;
        rows.extend(
            stmt.query_map(params![dir, cap as i64], raw)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    rows.sort_by(|a, b| {
        b.size(basis)
            .cmp(&a.size(basis))
            .then_with(|| a.key.reference().cmp(&b.key.reference()))
    });
    rows.truncate(cap);
    Ok(rows)
}
// Every early return removes the progress handler.
struct WorkGuard<'a> {
    conn: &'a Connection,
    hit: Arc<std::sync::atomic::AtomicBool>,
}
impl<'a> WorkGuard<'a> {
    fn new(conn: &'a Connection, budget: u64) -> Result<Self> {
        let remaining = Arc::new(AtomicU64::new(budget));
        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rem = remaining.clone();
        let h = hit.clone();
        let granularity = budget.clamp(1, 1000) as i32;
        conn.progress_handler(
            granularity,
            Some(move || {
                let old = rem.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    n.checked_sub(granularity as u64)
                });
                if old.is_err() {
                    h.store(true, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            }),
        )?;
        Ok(Self { conn, hit })
    }
    fn hit(&self) -> bool {
        self.hit.load(Ordering::Relaxed)
    }
}
impl Drop for WorkGuard<'_> {
    fn drop(&mut self) {
        let _ = self.conn.progress_handler(0, None::<fn() -> bool>);
    }
}

impl Reader {
    pub(crate) fn open(dir: &Path) -> Result<Self> {
        let conn =
            Connection::open_with_flags(dir.join("catalog.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.execute(
            "ATTACH DATABASE ?1 AS st",
            [dir.join("state.db").to_string_lossy().as_ref()],
        )?;
        conn.execute_batch("PRAGMA query_only=ON;PRAGMA foreign_keys=ON;PRAGMA cache_size=-16384;PRAGMA st.cache_size=-16384")?;
        Ok(Self {
            conn,
            breakdowns: HashMap::new(),
        })
    }
    pub(crate) fn read<T>(&mut self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let tx = self.conn.transaction()?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn children(&mut self, req: &ChildrenRequest) -> Result<ChildrenPage> {
        if !(1..=200).contains(&req.limit) {
            return Err(Error::Invalid("page limit"));
        }
        self.read(|conn| {
            get(conn,NodeKey::Dir(req.dir_id))?;
            let rev:i64=conn.query_row("SELECT listing_rev FROM dir WHERE id=?1",[req.dir_id],|r|r.get(0))?;
            if let Some(c)=&req.cursor {
                if c.dir_id!=req.dir_id || c.sort!=req.sort || c.basis!=req.basis {return Err(Error::Invalid("cursor binding"));}
                if c.listing_rev!=rev {return Err(Error::StaleGeneration);}
            }
            let (key,order)=match req.sort {Sort::SizeDesc=>(if req.basis==Basis::Logical{"logical"}else{"allocated"},"DESC"),Sort::NameAsc=>("name","ASC"),Sort::ModifiedDesc=>("coalesce(modified_ft,-1)","DESC")};
            let cmp=if order=="ASC"{">"}else{"<"};
            let after=if req.cursor.is_some(){format!("AND ({key} {cmp} ?3 OR ({key}=?3 AND (kind>?4 OR (kind=?4 AND id>?5))))")}else{String::new()};
            let sql=format!("SELECT * FROM (SELECT {DIR_COLUMNS} FROM dir d WHERE d.parent_id=?1 UNION ALL SELECT {FILE_COLUMNS} FROM {FILE_FROM} WHERE f.dir_id=?1) WHERE 1=1 {after} ORDER BY {key} {order},kind,id LIMIT ?2");
            let mut stmt=conn.prepare_cached(&sql)?;
            let mut values=vec![Value::Integer(req.dir_id),Value::Integer((req.limit+1) as i64)];
            if let Some(c)=&req.cursor {values.push(match &c.value {CursorValue::Number(n)=>Value::Integer(*n),CursorValue::Text(t)=>Value::Text(t.clone())});values.extend([Value::Integer(c.kind as i64),Value::Integer(c.row_id)]);}
            let mut rows=stmt.query_map(rusqlite::params_from_iter(values),raw)?.collect::<rusqlite::Result<Vec<_>>>()?;
            let more=rows.len()>req.limit;rows.truncate(req.limit);
            let next_cursor=if more {rows.last().map(|r|{let (kind,row_id)=match r.key {NodeKey::Dir(i)=>(0,i),NodeKey::File(i)=>(1,i),_=>unreachable!()};ChildrenCursor{dir_id:req.dir_id,listing_rev:rev,sort:req.sort,basis:req.basis,value:match req.sort {Sort::SizeDesc=>CursorValue::Number(r.size(req.basis) as i64),Sort::NameAsc=>CursorValue::Text(r.name.clone()),Sort::ModifiedDesc=>CursorValue::Number(r.modified.unwrap_or(-1))},kind,row_id}})}else{None};
            Ok(ChildrenPage{anchor:Some(NodeKey::Dir(req.dir_id).reference()),generation:Some(rev.to_string()),items:rows.iter().map(Raw::entry).collect(),next_cursor,total:None,budget_hit:false})
        })
    }
    pub fn path(&mut self, key: NodeKey) -> Result<NodePath> {
        self.read(|conn| node_path(conn, key))
    }
    pub fn inspect(&mut self, key: NodeKey) -> Result<NodeDetail> {
        self.read(|conn|{
            let r=get(conn,key)?;let path=node_path(conn,key)?;
            let (display,volume,volume_key):(String,Option<i64>,Option<String>)=conn.query_row("SELECT g.display_path,g.volume_id,v.volume_key FROM root_grant g LEFT JOIN volume v ON v.id=g.volume_id WHERE g.id=?1",[r.grant],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let mut display_path=display;
            for p in path.ancestors.iter().skip(1) {display_path.push('\\');display_path.push_str(&p.name);}
            if display_path.chars().count()>32768 {return Err(Error::Invalid("display path limit"));}
            let generation=revisions(conn,Some(r.grant))?[0].1.to_string();
            let identity=IdentityObservation{quality:if r.file_id.is_some(){"native_file_id_128"}else{"path_observation"}.into(),volume_key:volume_key.clone(),file_id_hex:r.file_id.as_ref().map(|id|id.iter().map(|v|format!("{v:02x}")).collect()),observed_generation:Some(generation),authorises_effects:false};
            let mut memberships=Vec::new();
            if let (Some(v),Some(id))=(&volume_key,&r.file_id) {
                let mut s=conn.prepare("SELECT c.id,c.name FROM st.collection c JOIN st.collection_member m ON m.collection_id=c.id JOIN st.object_ref o ON o.id=m.object_ref_id WHERE o.volume_key=?1 AND o.file_id=?2 LIMIT 32")?;
                memberships=s.query_map(params![v,id],|r|Ok(CollectionRef{collection_id:format!("cl_{}",r.get::<_,i64>(0)?),name:r.get(1)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
            }
            let subtree=if matches!(key,NodeKey::Dir(_)) {let (skipped,failed):(i64,i64)=conn.query_row("SELECT sub_skipped,sub_errors FROM dir WHERE id=?1",[match key{NodeKey::Dir(id)=>id,_=>unreachable!()}],|r|Ok((r.get(0)?,r.get(1)?)))?;Some(SubtreeTotals{files:r.totals.files,dirs:r.totals.dirs,logical_bytes:r.totals.logical.to_string(),allocated_bytes:r.entry().allocated_bytes,allocation_unknown_files:r.totals.allocation_unknown,skipped:skipped as u64,failed:failed as u64,complete:r.totals.complete})}else{None};
            let exclusions=flags(r.flags).into_iter().filter(|s|s!="hardlink_suspected").map(|code|Exclusion{explanation:code.replace('_'," "),code}).collect();
            let denied_capabilities=["file_move","file_delete","file_rename","content_read","teacher_disclosure","training"].into_iter().map(|capability|DeniedCapability{capability:capability.into(),reason:if r.flags&1!=0 && matches!(capability,"teacher_disclosure"|"training"){"sensitive_name"}else{"not_implemented_in_v03"}.into()}).collect();
            Ok(NodeDetail{row:r.entry(),root_id:format!("rt_{}",r.grant),volume_id:volume.map(|v|format!("vo_{v}")),display_path:DisplayPath::bounded(display_path,path.truncated),identity,timestamps:Timestamps{created_at:timestamp(r.created),modified_at:timestamp(r.modified),changed_at:timestamp(r.changed),accessed_at:timestamp(r.accessed),accessed_note:"Last access may be disabled or delayed; it is not usage evidence.".into()},subtree,threads:threads(conn,r.grant,&r.coverage(),r.flags)?,exclusions,denied_capabilities,memberships,suggestion:None})
        })
    }
    pub fn search(&mut self, req: &SearchRequest) -> Result<SearchPage> {
        if !(1..=100).contains(&req.limit)
            || req.text.chars().count() > 256
            || req
                .extension
                .as_ref()
                .is_some_and(|s| s.chars().count() > 32)
            || req.min_bytes.is_some_and(|v| v > i64::MAX as u64)
            || !matches!(req.kind.as_str(), "any" | "file" | "dir")
        {
            return Err(Error::Invalid("search bounds"));
        }
        self.read(|conn|{
            let revisions=revisions(conn,req.root_id)?;
            if let Some(c)=&req.cursor {if c.text!=req.text || c.extension!=req.extension || c.min_bytes!=req.min_bytes || c.kind!=req.kind || c.root_id!=req.root_id {return Err(Error::Invalid("search cursor binding"));}
if c.revisions!=revisions{return Err(Error::StaleGeneration);}}
            let guard=WorkGuard::new(conn,req.work_budget)?;
            let mut rows=Vec::new();let mut hit=false;
            let result=(||->Result<()> {
                for (kind,table,columns,from) in [(0,"d",DIR_COLUMNS,"dir d"),(1,"f",FILE_COLUMNS,FILE_FROM)] {
                    if (req.kind=="file" && kind==0)||(req.kind=="dir" && kind==1)||req.cursor.as_ref().is_some_and(|c|kind<c.last_kind){continue;}
                    let after=req.cursor.as_ref().filter(|c|c.last_kind==kind).map_or(0,|c|c.last_id);
                    let ext_filter=if kind==0 {"?4 IS NULL"}else{"(?4 IS NULL OR e.ext=?4)"};
                    let size=if kind==0 {"d.sub_logical"}else{"f.logical"};
                    let sql=format!("SELECT {columns} FROM {from} JOIN root_grant g ON g.id=d.grant_id WHERE g.state='active' AND (?1 IS NULL OR d.grant_id=?1) AND instr(lower({table}.name),lower(?2))>0 AND {table}.id>?3 AND {ext_filter} AND (?5 IS NULL OR {size}>=?5) ORDER BY {table}.id LIMIT ?6");
                    let mut stmt=conn.prepare_cached(&sql)?;
                    let mut iter=stmt.query(params![req.root_id,req.text,after,req.extension,req.min_bytes.map(|v|v as i64),(req.limit+1-rows.len()) as i64])?;
                    while let Some(row)=iter.next()? {rows.push(raw(row)?);}
                    if rows.len()>req.limit {break;}
                }Ok(())
            })();
            if let Err(e)=result {if guard.hit(){hit=true;}else{return Err(e);}}
            hit |= guard.hit();drop(guard);
            let more=rows.len()>req.limit;rows.truncate(req.limit);
            let next_cursor=if more||hit {rows.last().map(|r|{let (last_kind,last_id)=match r.key{NodeKey::Dir(id)=>(0,id),NodeKey::File(id)=>(1,id),_=>unreachable!()};SearchCursor{revisions:revisions.clone(),text:req.text.clone(),extension:req.extension.clone(),min_bytes:req.min_bytes,kind:req.kind.clone(),root_id:req.root_id,last_kind,last_id}})}else{None};
            let mut items=Vec::new();for r in rows {let mut row=r.entry();if let Some(parent)=r.parent {let p=node_path(conn,NodeKey::Dir(parent))?;row.location_hint=Some(DisplayPath::bounded(p.ancestors.iter().map(|p|p.name.as_str()).collect::<Vec<_>>().join("\\"),p.truncated));}items.push(row);}
            Ok(SearchPage{anchor:None,generation:if revisions.len()==1{Some(revisions[0].1.to_string())}else{None},items,next_cursor,total:None,budget_hit:hit})
        })
    }
    pub fn breakdown(
        &mut self,
        dir: i64,
        by: &str,
        basis: Basis,
        limit: usize,
        work_budget: u64,
    ) -> Result<Breakdown> {
        if !(1..=64).contains(&limit) || !matches!(by, "ext_family" | "extension" | "age_band") {
            return Err(Error::Invalid("breakdown bounds"));
        }
        let tx = self.conn.transaction()?;
        let conn: &Connection = &tx;
        let root = get(conn, NodeKey::Dir(dir))?;
        let generation = revisions(conn, Some(root.grant))?[0].1.to_string();
        let cache_key = (dir, by.to_string(), basis, limit, generation.clone());
        if by != "age_band" && !live(conn, root.grant)? {
            if let Some(cached) = self.breakdowns.get(&cache_key) {
                let cached = cached.clone();
                tx.commit()?;
                return Ok(cached);
            }
        }
        let now_ft = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 100
            + 116_444_736_000_000_000) as i64;
        let group=match by {"ext_family"=>"coalesce(e.family,'none')","extension"=>"coalesce(e.ext,'none')",_=>"CASE WHEN f.modified_ft IS NULL THEN 'unknown' WHEN f.modified_ft>=?2 THEN 'recent_30d' WHEN f.modified_ft>=?3 THEN 'within_year' ELSE 'older' END"};
        let bytes = if basis == Basis::Logical {
            "f.logical"
        } else {
            "coalesce(f.allocated,0)"
        };
        let unknown = if basis == Basis::Allocated {
            "f.allocated IS NULL OR "
        } else {
            ""
        };
        let sql=format!("WITH RECURSIVE tree(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM dir d JOIN tree t ON d.parent_id=t.id) SELECT {group} bucket,sum({bytes}),count(*),({unknown}({group})='unknown') unknown FROM file f JOIN tree t ON t.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id GROUP BY bucket,unknown ORDER BY sum({bytes}) DESC");
        let guard = WorkGuard::new(conn, work_budget)?;
        let mut buckets = Vec::new();
        let mut unknown_bytes = 0_u64;
        let mut unknown_files = 0_u64;
        let mut other_bytes = 0_u64;
        let mut other_files = 0_u64;
        let result = (|| -> Result<()> {
            let mut stmt = conn.prepare_cached(&sql)?;
            let mut iter = if by == "age_band" {
                stmt.query(params![
                    dir,
                    now_ft - 30 * 86400 * 10_000_000_i64,
                    now_ft - 365 * 86400 * 10_000_000_i64
                ])?
            } else {
                stmt.query([dir])?
            };
            while let Some(row) = iter.next()? {
                let key: String = row.get(0)?;
                let bytes = row.get::<_, i64>(1)? as u64;
                let files = row.get::<_, i64>(2)? as u64;
                let unknown: bool = row.get(3)?;
                if unknown {
                    unknown_bytes += bytes;
                    unknown_files += files;
                } else if buckets.len() < limit {
                    buckets.push(Bucket {
                        label: key.clone(),
                        key,
                        bytes: bytes.to_string(),
                        files,
                    });
                } else {
                    other_bytes += bytes;
                    other_files += files;
                }
            }
            Ok(())
        })();
        let hit = guard.hit();
        drop(guard);
        if let Err(e) = result {
            if !hit {
                return Err(e);
            }
        }
        if hit {
            let seen_bytes = buckets
                .iter()
                .map(|b| b.bytes.parse::<u64>().unwrap())
                .sum::<u64>()
                + unknown_bytes
                + other_bytes;
            let seen_files =
                buckets.iter().map(|b| b.files).sum::<u64>() + unknown_files + other_files;
            unknown_bytes += root.size(basis).saturating_sub(seen_bytes);
            unknown_files += root.totals.files.saturating_sub(seen_files);
        }
        let breakdown = Breakdown {
            node_id: NodeKey::Dir(dir).reference(),
            by: by.into(),
            basis,
            generation: Some(generation),
            complete: root.totals.complete && !hit,
            buckets,
            other: BytesAndFiles {
                bytes: other_bytes.to_string(),
                files: other_files,
            },
            unknown: BytesAndFiles {
                bytes: unknown_bytes.to_string(),
                files: unknown_files,
            },
        };
        tx.commit()?;
        if breakdown.complete && by != "age_band" {
            if self.breakdowns.len() >= 64 {
                self.breakdowns.clear();
            }
            self.breakdowns.insert(cache_key, breakdown.clone());
        }
        Ok(breakdown)
    }
}
fn node_path(conn: &Connection, key: NodeKey) -> Result<NodePath> {
    let mut current = Some(get(conn, key)?);
    let mut parts = Vec::new();
    let mut grant = None;
    while let Some(r) = current {
        grant = Some(r.grant);
        parts.push(PathPart {
            node_id: r.key.reference(),
            kind: if matches!(r.key, NodeKey::File(_)) {
                "file"
            } else if r.parent.is_none() {
                "root"
            } else {
                "dir"
            }
            .into(),
            name: r.name,
        });
        if parts.len() == 512 {
            parts.reverse();
            return Ok(NodePath {
                root_id: grant.map(|id| format!("rt_{id}")),
                ancestors: parts,
                truncated: r.parent.is_some(),
            });
        }
        current = match r.parent {
            Some(id) => Some(get(conn, NodeKey::Dir(id))?),
            None => None,
        };
    }
    parts.reverse();
    Ok(NodePath {
        root_id: grant.map(|id| format!("rt_{id}")),
        ancestors: parts,
        truncated: false,
    })
}
