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
    pub(crate) writer: Option<Arc<Writer>>,
    pub live_clamp_count: u64,
    breakdowns: HashMap<(i64, String, Basis, usize, String), Breakdown>,
}
pub(crate) const DIR_COLUMNS:&str="d.id,0 kind,d.name,d.sub_logical logical,d.sub_allocated allocated,d.sub_alloc_unknown unknown,d.sub_files files,d.sub_dirs dirs,d.modified_ft,d.attrs,d.flags,NULL extension,NULL family,CASE d.listing_state WHEN 'unlisted' THEN 'unscanned' WHEN 'incomplete' THEN 'partial' ELSE d.listing_state END,d.sub_complete complete,d.parent_id,d.root_id,d.file_id,d.created_ft,d.changed_ft,NULL accessed_ft,d.born_run,d.agg_valid_rev>=d.dirty_rev valid";
pub(crate) const FILE_COLUMNS:&str="f.id,1 kind,f.name,f.logical,coalesce(f.allocated,0) allocated,(f.allocated IS NULL) unknown,1 files,0 dirs,f.modified_ft,f.attrs,f.flags,e.ext extension,e.family,CASE WHEN d.listing_state='incomplete' AND f.seen_run!=d.seen_run THEN 'stale' ELSE CASE d.listing_state WHEN 'unlisted' THEN 'unscanned' WHEN 'incomplete' THEN 'partial' ELSE d.listing_state END END,1 complete,f.dir_id parent_id,d.root_id,f.file_id,f.created_ft,f.changed_ft,f.accessed_ft,f.born_run,1 valid";
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
    pub born_run: i64,
    pub valid: bool,
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
        born_run: r.get(21)?,
        valid: r.get(22)?,
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
        } else if matches!(self.key, NodeKey::File(_)) && self.state != "stale" {
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
    let sql=match key {NodeKey::Dir(_)=>format!("SELECT {DIR_COLUMNS} FROM dir d JOIN root_grant g ON g.id=d.root_id WHERE d.id=?1 AND g.state='active' AND d.listing_state!='absent_pending'"),NodeKey::File(_)=>format!("SELECT {FILE_COLUMNS} FROM {FILE_FROM} JOIN root_grant g ON g.id=d.root_id WHERE f.id=?1 AND g.state='active' AND d.listing_state!='absent_pending'"),_=>return Err(Error::Invalid("entry node expected"))};
    let id = match key {
        NodeKey::Dir(id) | NodeKey::File(id) => id,
        _ => unreachable!(),
    };
    let row = conn
        .query_row(&sql, [id], raw)
        .optional()?
        .ok_or(Error::NotFound)?;
    let directory = match row.key {
        NodeKey::Dir(id) => id,
        NodeKey::File(_) => row.parent.ok_or(Error::NotFound)?,
        _ => return Err(Error::NotFound),
    };
    let hidden:bool=conn.query_row("WITH RECURSIVE a(id,parent_id,listing_state) AS (SELECT id,parent_id,listing_state FROM dir WHERE id=?1 UNION ALL SELECT d.id,d.parent_id,d.listing_state FROM dir d JOIN a ON d.id=a.parent_id) SELECT EXISTS(SELECT 1 FROM a WHERE listing_state='absent_pending')",[directory],|r|r.get(0))?;
    if hidden {
        return Err(Error::NotFound);
    }
    Ok(row)
}
pub(crate) fn revisions(conn: &Connection, grant: Option<i64>) -> Result<Vec<(i64, Option<i64>)>> {
    let mut stmt=conn.prepare("SELECT d.root_id,r.generation FROM dir d JOIN root_grant g ON g.id=d.root_id JOIN root r ON r.id=d.root_id WHERE d.parent_id IS NULL AND g.state='active' AND d.listing_state!='absent_pending' AND (?1 IS NULL OR d.root_id=?1) ORDER BY d.root_id LIMIT 65")?;
    let rows = stmt
        .query_map([grant], |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?)))?
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
        "SELECT EXISTS(SELECT 1 FROM scan_run WHERE root_id=?1 AND state='running')",
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
        (
            dsize,
            if basis == Basis::Logical {
                "f.logical"
            } else {
                "f.allocated"
            },
            limit,
        )
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {DIR_COLUMNS} FROM dir d WHERE d.parent_id=?1 AND d.listing_state!='absent_pending' ORDER BY {dorder} DESC,d.id LIMIT ?2"
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
        (basis == Basis::Allocated && unknown_file(a))
            .cmp(&(basis == Basis::Allocated && unknown_file(b)))
            .then_with(|| {
                b.size(basis)
                    .cmp(&a.size(basis))
                    .then_with(|| row_key(a).cmp(&row_key(b)))
            })
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
                // A CAS loop rather than fetch_update, which newer stable toolchains deprecate.
                let mut n = rem.load(Ordering::Relaxed);
                let exhausted = loop {
                    let Some(next) = n.checked_sub(granularity as u64) else {
                        break true;
                    };
                    match rem.compare_exchange_weak(n, next, Ordering::Relaxed, Ordering::Relaxed) {
                        Ok(_) => break false,
                        Err(current) => n = current,
                    }
                };
                if exhausted {
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
        crate::db::grant_view(&conn)?;
        conn.execute_batch("PRAGMA query_only=ON;PRAGMA foreign_keys=ON;PRAGMA cache_size=-16384;PRAGMA st.cache_size=-16384")?;
        Ok(Self {
            conn,
            writer: None,
            live_clamp_count: 0,
            breakdowns: HashMap::new(),
        })
    }
    pub(crate) fn read<T>(&mut self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.repair_committed()?;
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
            let parent=get(conn,NodeKey::Dir(req.dir_id))?;
            let rev:i64=conn.query_row("SELECT subtree_rev FROM dir WHERE id=?1",[req.dir_id],|r|r.get(0))?;
            let instance=instance(conn)?;
            if let Some(c)=&req.cursor {
                if c.dir_id!=req.dir_id||c.sort!=req.sort||c.basis!=req.basis{return Err(Error::Invalid("cursor binding"));}
                if c.subtree_rev!=rev||c.catalog_instance!=instance{return Err(Error::StaleGeneration);}
            }
            if !parent.valid {return Err(Error::RepairRequired);}
            let bounded=req.sort!=Sort::SizeDesc;
            if bounded && conn.query_row("SELECT (SELECT count(*) FROM file WHERE dir_id=?1)+(SELECT count(*) FROM dir WHERE parent_id=?1 AND listing_state!='absent_pending')",[req.dir_id],|r|r.get::<_,i64>(0))?>10_000{return Err(Error::Invalid("sort exceeds 10000 children"));}
            let mut rows=Vec::new();
            for (kind,columns,from,owner,key) in [(0,DIR_COLUMNS,"dir d","d.parent_id",if req.basis==Basis::Logical{"d.sub_logical"}else{"d.sub_allocated"}),(1,FILE_COLUMNS,FILE_FROM,"f.dir_id",if req.basis==Basis::Logical{"f.logical"}else{"f.allocated"})] {
                if req.cursor.as_ref().is_some_and(|c|matches!(c.value,CursorValue::Unknown))&&kind==0&&!bounded{continue;}
                let mut values=vec![Value::Integer(req.dir_id),Value::Integer(if bounded{10001}else{(req.limit+1) as i64})];
                let after=if !bounded {if let Some(c)=&req.cursor {
                    let n=match c.value{CursorValue::Number(n)=>Value::Integer(n),CursorValue::Unknown=>Value::Null,_=>return Err(Error::Invalid("cursor value"))};
                    values.extend([n,Value::Integer(c.kind as i64),Value::Integer(c.row_id)]);
                    if kind==1 && req.basis==Basis::Allocated {
                        if matches!(c.value,CursorValue::Unknown){"AND f.allocated IS NULL AND f.id>?5 AND ?3 IS NULL AND ?4=1".into()}
                        else{format!("AND ({key}<?3 OR ({key}=?3 AND ({kind}>?4 OR ({kind}=?4 AND f.id>?5))) OR {key} IS NULL)")}
                    }
                    else {format!("AND ({key}<?3 OR ({key}=?3 AND ({kind}>?4 OR ({kind}=?4 AND {}.id>?5))))",if kind==0{"d"}else{"f"})}
                }else{String::new()}}else{String::new()};
                let sql=format!("SELECT {columns} FROM {from} WHERE {owner}=?1 AND d.listing_state!='absent_pending' {after} ORDER BY {key} DESC,{}.id LIMIT ?2",if kind==0{"d"}else{"f"});
                let mut stmt=conn.prepare_cached(&sql)?;
                rows.extend(stmt.query_map(rusqlite::params_from_iter(values),raw)?.collect::<rusqlite::Result<Vec<_>>>()?);
            }
            if rows.iter().any(|r|!r.valid){return Err(Error::RepairRequired);}
            rows.sort_by(|a,b|compare(a,b,req.sort,req.basis));
            if bounded {if let Some(c)=&req.cursor{rows.retain(|r|after_cursor(r,c));}}
            let more=rows.len()>req.limit;rows.truncate(req.limit);
            let next_cursor=if more{rows.last().map(|r|{let(kind,row_id)=row_key(r);ChildrenCursor{dir_id:req.dir_id,subtree_rev:rev,catalog_instance:instance,sort:req.sort,basis:req.basis,value:cursor_value(r,req.sort,req.basis),kind,row_id}})}else{None};
            Ok(ChildrenPage{anchor:Some(NodeKey::Dir(req.dir_id).reference()),generation:revisions(conn,Some(parent.grant))?.first().and_then(|(_,g)|g.map(|g|g.to_string())),items:rows.iter().map(Raw::entry).collect(),next_cursor,total:None,budget_hit:false})
        })
    }
    pub fn path(&mut self, key: NodeKey) -> Result<NodePath> {
        self.read(|conn| node_path(conn, key))
    }
    pub fn inspect(&mut self, key: NodeKey) -> Result<NodeDetail> {
        self.read(|conn|{
            let r=get(conn,key)?;if !r.valid{return Err(Error::RepairRequired);}let path=node_path(conn,key)?;
            let (display,volume,volume_key):(String,Option<i64>,Option<String>)=conn.query_row("SELECT g.display_path,g.volume_id,v.volume_key FROM root_grant g LEFT JOIN volume v ON v.id=g.volume_id WHERE g.id=?1",[r.grant],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let mut display_path=display;
            for p in path.ancestors.iter().skip(1) {display_path.push('\\');display_path.push_str(&p.name);}
            if display_path.chars().count()>32768 {return Err(Error::Invalid("display path limit"));}
            let generation=revisions(conn,Some(r.grant))?[0].1.map(|g|g.to_string());
            let identity=IdentityObservation{quality:match r.file_id.as_ref().map(Vec::len){Some(16)=>"native_file_id_128",Some(8)=>"native_file_id_64",_=>"path_observation"}.into(),volume_key:volume_key.clone(),file_id_hex:r.file_id.as_ref().map(|id|id.iter().map(|v|format!("{v:02x}")).collect()),observed_generation:generation,authorises_effects:false,durable_reference:durable_quality(conn,&r)?};
            let mut memberships=Vec::new();
            if let (Some(v),Some(id))=(&volume_key,&r.file_id) {
                let mut s=conn.prepare("SELECT c.id,c.name FROM st.collection c JOIN st.collection_member m ON m.collection_id=c.id JOIN st.object_ref o ON o.id=m.object_ref_id WHERE o.volume_key=?1 AND o.file_id=?2 AND o.state='resolved' AND o.retired=0 AND o.catalog_instance=?3 AND o.row_id=?4 AND o.born_run=?5 AND o.kind=?6 LIMIT 32")?;
                memberships=s.query_map(params![v,id,instance(conn)?,match r.key{NodeKey::Dir(id)|NodeKey::File(id)=>id,_=>0},r.born_run,if matches!(r.key,NodeKey::Dir(_)){"dir"}else{"file"}],|r|Ok(CollectionRef{collection_id:format!("cl_{}",r.get::<_,i64>(0)?),name:r.get(1)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
            }
            let subtree=if matches!(key,NodeKey::Dir(_)) {let (skipped,failed):(i64,i64)=conn.query_row("SELECT sub_skipped,sub_errors FROM dir WHERE id=?1",[match key{NodeKey::Dir(id)=>id,_=>unreachable!()}],|r|Ok((r.get(0)?,r.get(1)?)))?;let(unique_objects,unique_allocated_bytes,multi_link_entries)=unique_totals(conn,&r)?;Some(SubtreeTotals{files:r.totals.files,dirs:r.totals.dirs,logical_bytes:r.totals.logical.to_string(),allocated_bytes:r.entry().allocated_bytes,allocation_unknown_files:r.totals.allocation_unknown,skipped:skipped as u64,failed:failed as u64,complete:r.totals.complete,stream_coverage:"default_stream_only".into(),unique_objects,unique_allocated_bytes,multi_link_entries})}else{None};
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
            let revisions=revisions(conn,req.root_id)?;let catalog_rev=catalog_rev(conn)?;let catalog_instance=instance(conn)?;
            if let Some(c)=&req.cursor {if c.text!=req.text || c.extension!=req.extension || c.min_bytes!=req.min_bytes || c.kind!=req.kind || c.root_id!=req.root_id {return Err(Error::Invalid("search cursor binding"));}
if c.catalog_rev!=catalog_rev||c.catalog_instance!=catalog_instance{return Err(Error::StaleGeneration);}}
            let guard=WorkGuard::new(conn,req.work_budget)?;
            let mut rows=Vec::new();let mut hit=false;
            let mut last_kind=req.cursor.as_ref().map_or(0,|c|c.last_kind);
            let mut last_id=req.cursor.as_ref().map_or(0,|c|c.last_id);
            let mut more=false;
            let result=(||->Result<()> {
                for (kind,table,columns,from,ids) in [(0,"d",DIR_COLUMNS,"dir d","dir"),(1,"f",FILE_COLUMNS,FILE_FROM,"file")] {
                    if (req.kind=="file"&&kind==0)||(req.kind=="dir"&&kind==1)||kind<last_kind{continue;}
                    if kind!=last_kind {last_kind=kind;last_id=0;}
                    loop {
                        let end:Option<i64>=conn.prepare_cached(&format!("SELECT max(id) FROM (SELECT id FROM {ids} WHERE id>?1 ORDER BY id LIMIT 128)"))?.query_row([last_id],|r|r.get(0))?;
                        let Some(end)=end else{break};
                        let ext_filter=if kind==0 {"?4 IS NULL"}else{"(?4 IS NULL OR e.ext=?4)"};
                        let size=if kind==0 {"d.sub_logical"}else{"f.logical"};
                        let sql=format!("SELECT {columns} FROM {from} JOIN root_grant g ON g.id=d.root_id WHERE g.state='active' AND d.id IN (SELECT id FROM visible_dir) AND (?1 IS NULL OR d.root_id=?1) AND instr(lower({table}.name),lower(?2))>0 AND {table}.id>?3 AND {table}.id<=?7 AND {ext_filter} AND (?5 IS NULL OR {size}>=?5) ORDER BY {table}.id LIMIT ?6");
                        let mut stmt=conn.prepare_cached(&sql)?;
                        let mut iter=stmt.query(params![req.root_id,req.text,last_id,req.extension,req.min_bytes.map(|v|v as i64),(req.limit+1-rows.len()) as i64,end])?;
                        while let Some(row)=iter.next()? {let row=raw(row)?;if !row.valid{return Err(Error::RepairRequired);}last_id=row_key(&row).1;rows.push(row);}
                        if rows.len()>req.limit{more=true;break;}
                        last_id=end;
                    }
                    if more{break;}
                }Ok(())
            })();
            if let Err(e)=result {if guard.hit(){hit=true;}else{return Err(e);}}
            hit|=guard.hit();drop(guard);
            if more{rows.truncate(req.limit);if let Some(row)=rows.last(){(last_kind,last_id)=row_key(row);}}
            let next_cursor=if more||hit {Some(SearchCursor{catalog_rev,catalog_instance:catalog_instance.clone(),text:req.text.clone(),extension:req.extension.clone(),min_bytes:req.min_bytes,kind:req.kind.clone(),root_id:req.root_id,last_kind,last_id})}else{None};
            let mut paths=HashMap::new();let mut items=Vec::new();for r in rows {let mut row=r.entry();if let Some(parent)=r.parent {let p=if let Some(p)=paths.get(&parent){p}else{paths.insert(parent,node_path(conn,NodeKey::Dir(parent))?);paths.get(&parent).unwrap()};row.location_hint=Some(DisplayPath::bounded(p.ancestors.iter().map(|p|p.name.as_str()).collect::<Vec<_>>().join("\\"),p.truncated));}items.push(row);}
            Ok(SearchPage{anchor:None,generation:if revisions.len()==1{revisions[0].1.map(|g|g.to_string())}else{None},items,next_cursor,total:None,budget_hit:hit})
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
        self.repair_committed()?;
        let tx = self.conn.transaction()?;
        let conn: &Connection = &tx;
        let root = get(conn, NodeKey::Dir(dir))?;
        let generation = revisions(conn, Some(root.grant))?[0]
            .1
            .map(|g| g.to_string());
        let subtree_rev: i64 =
            conn.query_row("SELECT subtree_rev FROM dir WHERE id=?1", [dir], |r| {
                r.get(0)
            })?;
        if !root.valid {
            return Err(Error::RepairRequired);
        }
        let cache_key = (dir, by.to_string(), basis, limit, subtree_rev.to_string());
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
        let sql=format!("WITH RECURSIVE tree(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM dir d JOIN tree t ON d.parent_id=t.id WHERE d.listing_state!='absent_pending') SELECT {group} bucket,sum({bytes}),count(*),({unknown}({group})='unknown') unknown FROM file f JOIN tree t ON t.id=f.dir_id LEFT JOIN ext e ON e.id=f.ext_id GROUP BY bucket,unknown ORDER BY sum({bytes}) DESC");
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
            generation,
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
    let row = get(conn, key)?;
    let mut parts = vec![PathPart {
        node_id: key.reference(),
        kind: if matches!(key, NodeKey::File(_)) {
            "file"
        } else if row.parent.is_none() {
            "root"
        } else {
            "dir"
        }
        .into(),
        name: row.name,
    }];
    let mut truncated = false;
    if let Some(parent) = row.parent {
        let mut stmt=conn.prepare_cached("WITH RECURSIVE a(id,parent_id,name,depth) AS (SELECT id,parent_id,name,1 FROM dir WHERE id=?1 UNION ALL SELECT d.id,d.parent_id,d.name,a.depth+1 FROM dir d JOIN a ON d.id=a.parent_id WHERE a.depth<511) SELECT id,parent_id,name FROM a ORDER BY depth")?;
        let rows = stmt
            .query_map([parent], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, parent, name) in rows {
            truncated = parent.is_some();
            parts.push(PathPart {
                node_id: NodeKey::Dir(id).reference(),
                kind: if parent.is_none() { "root" } else { "dir" }.into(),
                name,
            });
        }
    }
    parts.reverse();
    Ok(NodePath {
        root_id: Some(format!("rt_{}", row.grant)),
        ancestors: parts,
        truncated,
    })
}
fn row_key(r: &Raw) -> (u8, i64) {
    match r.key {
        NodeKey::Dir(id) => (0, id),
        NodeKey::File(id) => (1, id),
        _ => unreachable!(),
    }
}
fn cursor_value(r: &Raw, sort: Sort, basis: Basis) -> CursorValue {
    if sort == Sort::SizeDesc && basis == Basis::Allocated && unknown_file(r) {
        return CursorValue::Unknown;
    }
    match sort {
        Sort::SizeDesc => CursorValue::Number(r.size(basis) as i64),
        Sort::NameAsc => CursorValue::Text(r.name.clone()),
        Sort::ModifiedDesc => CursorValue::Number(r.modified.unwrap_or(-1)),
    }
}
fn unknown_file(r: &Raw) -> bool {
    matches!(r.key, NodeKey::File(_)) && r.totals.allocation_unknown > 0
}
fn compare(a: &Raw, b: &Raw, sort: Sort, basis: Basis) -> std::cmp::Ordering {
    let unknown = if sort == Sort::SizeDesc && basis == Basis::Allocated {
        unknown_file(a).cmp(&unknown_file(b))
    } else {
        std::cmp::Ordering::Equal
    };
    unknown
        .then_with(|| match sort {
            Sort::SizeDesc => b.size(basis).cmp(&a.size(basis)),
            Sort::NameAsc => a.name.cmp(&b.name),
            Sort::ModifiedDesc => b.modified.unwrap_or(-1).cmp(&a.modified.unwrap_or(-1)),
        })
        .then_with(|| row_key(a).cmp(&row_key(b)))
}
fn after_cursor(r: &Raw, c: &ChildrenCursor) -> bool {
    let order = match (cursor_value(r, c.sort, c.basis), &c.value) {
        (CursorValue::Number(a), CursorValue::Number(b)) => b.cmp(&a),
        (CursorValue::Text(a), CursorValue::Text(b)) => a.cmp(b),
        _ => return false,
    };
    order.is_gt() || (order.is_eq() && row_key(r) > (c.kind, c.row_id))
}
pub(crate) fn instance(conn: &Connection) -> Result<String> {
    Ok(
        conn.query_row("SELECT value FROM meta WHERE key='instance_id'", [], |r| {
            r.get(0)
        })?,
    )
}
fn catalog_rev(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT catalog_rev FROM revision", [], |r| r.get(0))?)
}
fn durable_quality(conn: &Connection, r: &Raw) -> Result<String> {
    let fs: Option<String> = conn.query_row(
        "SELECT v.filesystem FROM root rt LEFT JOIN volume v ON v.id=rt.volume_id WHERE rt.id=?1",
        [r.grant],
        |r| r.get(0),
    )?;
    Ok(if !matches!(fs.as_deref(), Some("NTFS" | "ReFS")) {
        "unavailable_filesystem"
    } else if r.created.is_none()
        || r.file_id.is_none()
        || (fs.as_deref() == Some("ReFS") && r.file_id.as_ref().is_some_and(|id| id.len() != 16))
    {
        "unavailable_identity_quality"
    } else {
        "available"
    }
    .into())
}
fn unique_totals(conn: &Connection, r: &Raw) -> Result<(Option<u64>, Option<String>, u64)> {
    let id = match r.key {
        NodeKey::Dir(id) => id,
        _ => return Ok((None, None, 0)),
    };
    let guard = WorkGuard::new(conn, 2_000_000)?;
    let result=conn.query_row("WITH RECURSIVE tree(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM dir d JOIN tree t ON d.parent_id=t.id WHERE d.listing_state!='absent_pending'), objects AS (SELECT file_id,max(allocated) allocated,sum(allocated IS NULL) unknown,sum((flags & ?2)!=0 OR coalesce(link_count,0)>1) links,count(*) names FROM file WHERE dir_id IN (SELECT id FROM tree) GROUP BY file_id) SELECT count(file_id),coalesce(sum(allocated),0),coalesce(sum(unknown),0),coalesce(sum(file_id IS NULL),0),coalesce(sum(links),0) FROM objects",params![id,HARDLINK_SUSPECTED],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?)));
    match result {
        Ok((count, bytes, unknown, noid, links)) => Ok((
            (noid == 0).then_some(count as u64),
            (noid == 0 && unknown == 0).then(|| bytes.to_string()),
            links as u64,
        )),
        Err(_) if guard.hit() => Err(Error::ResourceBudget),
        Err(e) => Err(Error::Sql(e)),
    }
}
impl Reader {
    fn repair_committed(&mut self) -> Result<()> {
        let roots = {
            let mut s=self.conn.prepare("SELECT DISTINCT d.root_id FROM dir d JOIN root r ON r.id=d.root_id WHERE d.agg_valid_rev<d.dirty_rev AND r.active_run IS NULL AND d.parent_id IS NULL")?;
            let rows = s
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for root in roots {
            self.writer
                .as_ref()
                .ok_or(Error::RepairRequired)?
                .call(WriteCommand::Repair { root_id: root })?;
        }
        Ok(())
    }
    /// Internal incarnation binding; L8 adds the session HMAC before transport.
    pub fn incarnation(&mut self, key: NodeKey) -> Result<(String, i64)> {
        self.read(|conn| {
            let r = get(conn, key)?;
            Ok((instance(conn)?, r.born_run))
        })
    }
    pub fn check_incarnation(
        &mut self,
        key: NodeKey,
        instance_id: &str,
        born_run: i64,
    ) -> Result<()> {
        let (current, born) = self.incarnation(key)?;
        if current != instance_id || born != born_run {
            return Err(Error::NotFound);
        }
        Ok(())
    }
}
