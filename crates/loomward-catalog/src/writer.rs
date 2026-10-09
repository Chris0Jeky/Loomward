use crate::*;
use crossbeam_channel::{bounded, Receiver, Sender};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

const QUEUE_ENTRIES: usize = 200_000;
type Quota = Arc<(Mutex<usize>, Condvar)>;
struct Pending {
    command: WriteCommand,
    reply: Sender<Result<WriteReply>>,
    entries: usize,
}
pub struct Writer {
    tx: Option<Sender<Pending>>,
    quota: Quota,
    join: Option<JoinHandle<()>>,
}
pub struct Receipt {
    rx: Receiver<Result<WriteReply>>,
}
impl Receipt {
    pub fn wait(self) -> Result<WriteReply> {
        self.rx.recv().map_err(|_| Error::Closed)?
    }
}
impl Writer {
    pub(crate) fn start(conn: Connection) -> Result<Self> {
        let (tx, rx) = bounded(64);
        let quota = Arc::new((Mutex::new(0), Condvar::new()));
        let shared = quota.clone();
        let join = std::thread::Builder::new()
            .name("loomward-catalog".into())
            .spawn(move || work(conn, rx, shared))?;
        Ok(Self {
            tx: Some(tx),
            quota,
            join: Some(join),
        })
    }
    pub fn send(&self, command: WriteCommand) -> Result<Receipt> {
        let charge = validate(&command)?.min(QUEUE_ENTRIES);
        let mut queued = self.quota.0.lock().map_err(|_| Error::Closed)?;
        while *queued + charge > QUEUE_ENTRIES {
            queued = self.quota.1.wait(queued).map_err(|_| Error::Closed)?;
        }
        *queued += charge;
        drop(queued);
        let (reply, rx) = bounded(1);
        if self
            .tx
            .as_ref()
            .ok_or(Error::Closed)?
            .send(Pending {
                command,
                reply,
                entries: charge,
            })
            .is_err()
        {
            release(&self.quota, charge);
            return Err(Error::Closed);
        }
        Ok(Receipt { rx })
    }
    pub fn call(&self, command: WriteCommand) -> Result<WriteReply> {
        self.send(command)?.wait()
    }
    pub fn queued_entries(&self) -> usize {
        *self.quota.0.lock().expect("quota lock")
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
fn release(q: &Quota, n: usize) {
    let mut count = q.0.lock().expect("quota lock");
    *count -= n;
    q.1.notify_all();
}
fn validate(c: &WriteCommand) -> Result<usize> {
    match c {
        WriteCommand::DirListing(l) => {
            let n = l.files.len() + l.dirs.len();
            if n > 2_000_000
                || !STATES.contains(&l.state.as_str())
                || l.skipped > i64::MAX as u64
                || l.errors > i64::MAX as u64
            {
                return Err(Error::Invalid("listing bounds"));
            }
            let mut names = HashSet::new();
            let mut identities = HashSet::new();
            for o in l.files.iter().chain(&l.dirs) {
                o.validate()?;
                if !names.insert((&o.name, &o.name_utf16)) {
                    return Err(Error::Invalid("duplicate listing name"));
                }
            }
            if l.dirs
                .iter()
                .filter_map(|o| o.file_id)
                .any(|id| !identities.insert(id))
            {
                return Err(Error::Invalid("duplicate directory identity"));
            }
            Ok(n)
        }
        WriteCommand::DirFinal { totals: t, .. } => {
            if [
                t.files,
                t.dirs,
                t.logical,
                t.allocated,
                t.allocation_unknown,
                t.skipped,
                t.errors,
            ]
            .into_iter()
            .any(|v| v > i64::MAX as u64)
            {
                return Err(Error::Invalid("totals bounds"));
            }
            Ok(0)
        }
        _ => Ok(0),
    }
}
fn work(mut conn: Connection, rx: Receiver<Pending>, quota: Quota) {
    let mut pending = None;
    loop {
        let first = match pending.take().or_else(|| rx.recv().ok()) {
            Some(p) => p,
            None => break,
        };
        let checkpoint = matches!(first.command, WriteCommand::EndRun { .. });
        let mut batch = vec![first];
        if matches!(batch[0].command, WriteCommand::DirListing(_)) {
            let start = Instant::now();
            let mut rows = batch[0].entries;
            while rows < 50_000 && start.elapsed() < Duration::from_millis(250) {
                match rx.try_recv() {
                    Ok(p)
                        if matches!(p.command, WriteCommand::DirListing(_))
                            && rows + p.entries <= 50_000 =>
                    {
                        rows += p.entries;
                        batch.push(p);
                    }
                    Ok(p) => {
                        pending = Some(p);
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
        let mut results = Vec::new();
        match conn.transaction() {
            Ok(mut tx) => {
                for p in &batch {
                    results.push((|| {
                        let save = tx.savepoint()?;
                        let result = apply(&save, &p.command)?;
                        save.commit()?;
                        Ok(result)
                    })());
                }
                if let Err(e) = tx.commit() {
                    results = std::iter::repeat_with(|| Err(Error::Invalid("batch commit failed")))
                        .take(batch.len())
                        .collect();
                    results[0] = Err(Error::Sql(e));
                }
            }
            Err(e) => {
                results = std::iter::repeat_with(|| Err(Error::Closed))
                    .take(batch.len())
                    .collect();
                results[0] = Err(Error::Sql(e));
            }
        }
        if checkpoint {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        }
        for (p, result) in batch.into_iter().zip(results) {
            release(&quota, p.entries);
            let _ = p.reply.send(result);
        }
    }
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
}
fn active_run(conn: &Connection, run: i64, dir: Option<i64>) -> Result<i64> {
    let grant:Option<i64>=conn.query_row("SELECT s.grant_id FROM scan_run s JOIN root_grant g ON g.id=s.grant_id WHERE s.id=?1 AND s.state='running' AND g.state='active'",[run],|r|r.get(0)).optional()?;
    let grant = grant.ok_or(Error::Invalid("inactive run or grant"))?;
    if let Some(dir) = dir {
        if !conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM dir WHERE id=?1 AND grant_id=?2)",
            params![dir, grant],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::NotFound);
        }
    }
    Ok(grant)
}
fn next_id(conn: &Connection, table: &str) -> Result<i64> {
    reserve_ids(conn, table, 1)
}
fn reserve_ids(conn: &Connection, table: &str, count: usize) -> Result<i64> {
    let key = format!("next_{table}_id");
    let next: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
        .optional()?;
    let id = match next {
        Some(s) => s.parse::<i64>().map_err(|_| Error::CorruptDatabase)?,
        None => conn.query_row(
            &format!("SELECT coalesce(max(id),0)+1 FROM {table}"),
            [],
            |r| r.get(0),
        )?,
    };
    let future = id
        .checked_add(count as i64)
        .ok_or(Error::Invalid("row id exhausted"))?;
    conn.execute("INSERT INTO meta(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,future.to_string()])?;
    Ok(id)
}
fn apply(conn: &Connection, c: &WriteCommand) -> Result<WriteReply> {
    match c {
        WriteCommand::RegisterRoot(o) => {
            if o.volume_key.is_empty()
                || o.volume_key.len() > 80
                || o.display_name.chars().count() > 260
                || o.display_path.chars().count() > 32768
                || !matches!(
                    o.origin.as_str(),
                    "fixture" | "lab_generated" | "owner_granted"
                )
                || !matches!(
                    o.granted_via.as_str(),
                    "fixture" | "cli_flag" | "desktop_picker"
                )
            {
                return Err(Error::Invalid("root observation"));
            }
            conn.execute("INSERT INTO volume(volume_key,display_name,identity_json,capabilities_json,device_json,online,observed_at_ns) VALUES (?1,?2,'{}','{}','{}',1,?3) ON CONFLICT(volume_key) DO NOTHING",params![o.volume_key,o.display_name,o.observed_at_ns])?;
            let volume: i64 = conn.query_row(
                "SELECT id FROM volume WHERE volume_key=?1",
                [&o.volume_key],
                |r| r.get(0),
            )?;
            conn.execute("INSERT INTO root_grant(volume_id,root_file_id,display_path,origin,granted_via,state,granted_at_ns) VALUES (?1,?2,?3,?4,?5,'active',?6)",params![volume,o.root_file_id.as_ref().map(|id|id.as_slice()),o.display_path,o.origin,o.granted_via,o.observed_at_ns])?;
            let grant = conn.last_insert_rowid();
            let dir = next_id(conn, "dir")?;
            conn.execute("INSERT INTO dir(id,grant_id,parent_id,name,file_id,depth,attrs,state,seen_run) VALUES (?1,?2,NULL,?3,?4,0,0,'unscanned',0)",params![dir,grant,o.display_name,o.root_file_id.as_ref().map(|id|id.as_slice())])?;
            Ok(WriteReply::Root {
                grant_id: grant,
                dir_id: dir,
            })
        }
        WriteCommand::BeginRun {
            grant_id,
            mode,
            strategy,
            started_at_ns,
        } => {
            if !matches!(mode.as_str(), "full" | "refresh") || strategy.len() > 128 {
                return Err(Error::Invalid("run bounds"));
            }
            if !conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_grant WHERE id=?1 AND state='active')",
                [grant_id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::NotFound);
            }
            if conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM scan_run WHERE grant_id=?1 AND state='running')",
                [grant_id],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(Error::Invalid("run already active"));
            }
            conn.execute("INSERT INTO scan_run(grant_id,mode,strategy,state,started_at_ns,counters_json,budget_json) VALUES (?1,?2,?3,'running',?4,'{}','{}')",params![grant_id,mode,strategy,started_at_ns])?;
            Ok(WriteReply::Run(conn.last_insert_rowid()))
        }
        WriteCommand::DirListing(l) => listing(conn, l),
        WriteCommand::DirFinal {
            run_id,
            dir_id,
            totals: t,
        } => {
            active_run(conn, *run_id, Some(*dir_id))?;
            let changed:bool=conn.query_row("SELECT sub_files!=?2 OR sub_dirs!=?3 OR sub_logical!=?4 OR sub_allocated!=?5 OR sub_alloc_unknown!=?6 OR sub_skipped!=?7 OR sub_errors!=?8 OR sub_newest_ft IS NOT ?9 OR sub_complete!=?10 FROM dir WHERE id=?1",params![dir_id,t.files as i64,t.dirs as i64,t.logical as i64,t.allocated as i64,t.allocation_unknown as i64,t.skipped as i64,t.errors as i64,t.newest_ft,t.complete],|r|r.get(0))?;
            conn.execute("UPDATE dir SET sub_files=?2,sub_dirs=?3,sub_logical=?4,sub_allocated=?5,sub_alloc_unknown=?6,sub_skipped=?7,sub_errors=?8,sub_newest_ft=?9,sub_complete=?10 WHERE id=?1",params![dir_id,t.files as i64,t.dirs as i64,t.logical as i64,t.allocated as i64,t.allocation_unknown as i64,t.skipped as i64,t.errors as i64,t.newest_ft,t.complete])?;
            if changed {
                conn.execute("WITH RECURSIVE ancestors(id) AS (SELECT ?1 UNION ALL SELECT d.parent_id FROM dir d JOIN ancestors a ON d.id=a.id WHERE d.parent_id IS NOT NULL) UPDATE dir SET listing_rev=listing_rev+1 WHERE id IN (SELECT id FROM ancestors)",[dir_id])?;
            }
            Ok(WriteReply::Done)
        }
        WriteCommand::EndRun {
            run_id,
            state,
            finished_at_ns,
        } => {
            let grant = active_run(conn, *run_id, None)?;
            if !matches!(state.as_str(), "completed" | "cancelled" | "failed") {
                return Err(Error::Invalid("end state"));
            }
            if state != "completed" {
                conn.execute("UPDATE dir SET state=CASE (SELECT mode FROM scan_run WHERE id=?1) WHEN 'refresh' THEN 'stale' ELSE 'unscanned' END WHERE grant_id=?2 AND seen_run!=?1",params![run_id,grant])?;
            }
            conn.execute("UPDATE file SET flags=flags & ~?2 WHERE dir_id IN (SELECT id FROM dir WHERE grant_id=?1)",params![grant,HARDLINK_SUSPECTED])?;
            conn.execute("UPDATE file SET flags=flags | ?2 WHERE dir_id IN (SELECT id FROM dir WHERE grant_id=?1) AND file_id IN (SELECT f.file_id FROM file f JOIN dir d ON d.id=f.dir_id WHERE d.grant_id=?1 AND f.file_id IS NOT NULL GROUP BY f.file_id HAVING count(*)>1)",params![grant,HARDLINK_SUSPECTED])?;
            rollup(conn, grant, state == "completed")?;
            if state != "completed" {
                conn.execute(
                    "UPDATE dir SET state=?2 WHERE grant_id=?1 AND parent_id IS NULL",
                    params![
                        grant,
                        if state == "cancelled" {
                            "cancelled"
                        } else {
                            "partial"
                        }
                    ],
                )?;
            }
            conn.execute(
                "UPDATE scan_run SET state=?2,finished_at_ns=?3 WHERE id=?1",
                params![run_id, state, finished_at_ns],
            )?;
            Ok(WriteReply::Done)
        }
        WriteCommand::Barrier => Ok(WriteReply::Done),
    }
}
fn stored(conn: &Connection, table: &str, dir: i64) -> Result<Vec<(i64, Observation)>> {
    let (owner, extra, join) = if table == "file" {
        (
            "dir_id",
            "f.logical,f.allocated,e.ext,coalesce(e.family,'none'),f.accessed_ft",
            "LEFT JOIN ext e ON e.id=f.ext_id",
        )
    } else {
        ("parent_id", "0,NULL,NULL,'none',NULL", "")
    };
    let mut stmt=conn.prepare_cached(&format!("SELECT f.id,f.name,f.name_utf16,f.file_id,f.created_ft,f.modified_ft,f.changed_ft,f.attrs,f.reparse_tag,f.flags,{extra} FROM {table} f {join} WHERE f.{owner}=?1"))?;
    let rows = stmt.query_map([dir], |r| {
        let id: Option<Vec<u8>> = r.get(3)?;
        Ok((
            r.get(0)?,
            Observation {
                name: r.get(1)?,
                name_utf16: r.get(2)?,
                file_id: id.and_then(|v| v.try_into().ok()),
                created_ft: r.get(4)?,
                modified_ft: r.get(5)?,
                changed_ft: r.get(6)?,
                attrs: r.get(7)?,
                reparse_tag: r.get(8)?,
                flags: r.get(9)?,
                logical: r.get::<_, i64>(10)? as u64,
                allocated: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                extension: r.get(12)?,
                family: r.get(13)?,
                accessed_ft: r.get(14)?,
            },
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}
fn listing(conn: &Connection, l: &DirListing) -> Result<WriteReply> {
    let grant = active_run(conn, l.run_id, Some(l.dir_id))?;
    let depth: i64 = conn.query_row("SELECT depth FROM dir WHERE id=?1", [l.dir_id], |r| {
        r.get(0)
    })?;
    if depth >= 511 && !l.dirs.is_empty() {
        return Err(Error::Invalid("tree depth limit"));
    }
    let mut changed = false;
    for (table, incoming) in [("file", &l.files), ("dir", &l.dirs)] {
        let old = stored(conn, table, l.dir_id)?;
        let mut by_name = HashMap::new();
        let mut by_identity: HashMap<[u8; 16], Vec<usize>> = HashMap::new();
        for (i, (_, o)) in old.iter().enumerate() {
            by_name.insert((&o.name, &o.name_utf16), i);
            if let Some(id) = o.file_id {
                by_identity.entry(id).or_default().push(i);
            }
        }
        let mut used = HashSet::new();
        let mut matches = Vec::new();
        for o in incoming {
            let same = by_name
                .get(&(&o.name, &o.name_utf16))
                .copied()
                .filter(|i| old[*i].1.file_id == o.file_id);
            if let Some(i) = same {
                used.insert(i);
            }
            matches.push(same);
        }
        for (o, matched) in incoming.iter().zip(&mut matches) {
            if matched.is_none() {
                *matched = o
                    .file_id
                    .and_then(|id| by_identity.get(&id))
                    .and_then(|ids| ids.iter().copied().find(|i| !used.contains(i)));
                if matched.is_none() {
                    *matched = by_name.get(&(&o.name, &o.name_utf16)).copied().filter(|i| {
                        !used.contains(i) && (old[*i].1.file_id.is_none() || o.file_id.is_none())
                    });
                }
                if let Some(i) = *matched {
                    used.insert(i);
                }
            }
        }
        for (i, (id, _)) in old.iter().enumerate() {
            if !used.contains(&i) {
                conn.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id])?;
                changed = true;
            }
        }
        let mut next_new =
            reserve_ids(conn, table, matches.iter().filter(|m| m.is_none()).count())?;
        let mut extensions: HashMap<String, (i64, String)> = HashMap::new();
        for (o, matched) in incoming.iter().zip(matches) {
            let mut normal = o.clone();
            if table == "file" {
                normal.flags &= !HARDLINK_SUSPECTED;
            } else {
                normal.logical = 0;
                normal.allocated = None;
                normal.extension = None;
                normal.family = "none".into();
                normal.accessed_ft = None;
            }
            let equal = matched.is_some_and(|i| {
                let mut before = old[i].1.clone();
                if table == "file" {
                    before.flags &= !HARDLINK_SUSPECTED;
                }
                before == normal
            });
            if equal {
                continue;
            }
            changed = true;
            let mut id = match matched {
                Some(i) => old[i].0,
                None => {
                    let id = next_new;
                    next_new += 1;
                    id
                }
            };
            let file_id = o.file_id.as_ref().map(|v| v.as_slice());
            if table == "dir" && matched.is_none() {
                if let Some(file_id) = file_id {
                    let existing: Option<(i64, Option<i64>, i64)> = conn
                        .query_row(
                            "SELECT id,parent_id,depth FROM dir WHERE grant_id=?1 AND file_id=?2",
                            params![grant, file_id],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                        )
                        .optional()?;
                    if let Some((existing, old_parent, old_depth)) = existing {
                        let old_parent =
                            old_parent.ok_or(Error::Invalid("cannot reparent a grant root"))?;
                        let cycle:bool=conn.query_row("WITH RECURSIVE ancestors(id) AS (SELECT ?1 UNION ALL SELECT d.parent_id FROM dir d JOIN ancestors a ON d.id=a.id WHERE d.parent_id IS NOT NULL) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=?2)",params![l.dir_id,existing],|r|r.get(0))?;
                        if cycle {
                            return Err(Error::Invalid("directory identity cycle"));
                        }
                        let delta = depth + 1 - old_depth;
                        let deepest:i64=conn.query_row("WITH RECURSIVE tree(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM dir d JOIN tree t ON d.parent_id=t.id) SELECT max(depth) FROM dir WHERE id IN (SELECT id FROM tree)",[existing],|r|r.get(0))?;
                        if deepest + delta > 511 {
                            return Err(Error::Invalid("tree depth limit"));
                        }
                        conn.execute("WITH RECURSIVE tree(id) AS (SELECT ?1 UNION ALL SELECT d.id FROM dir d JOIN tree t ON d.parent_id=t.id) UPDATE dir SET depth=depth+?2 WHERE id IN (SELECT id FROM tree)",params![existing,delta])?;
                        conn.execute("WITH RECURSIVE ancestors(id) AS (SELECT ?1 UNION ALL SELECT d.parent_id FROM dir d JOIN ancestors a ON d.id=a.id WHERE d.parent_id IS NOT NULL) UPDATE dir SET listing_rev=listing_rev+1,sub_complete=0 WHERE id IN (SELECT id FROM ancestors)",[old_parent])?;
                        id = existing;
                    }
                }
            }
            if table == "file" {
                let ext = if let Some(ext) = &o.extension {
                    if let Some((id, family)) = extensions.get(ext) {
                        if family != &o.family {
                            return Err(Error::Invalid("conflicting extension family"));
                        }
                        Some(*id)
                    } else {
                        conn.execute("INSERT INTO ext(ext,family) VALUES (?1,?2) ON CONFLICT(ext) DO NOTHING",params![ext,o.family])?;
                        let (id, family): (i64, String) =
                            conn.query_row("SELECT id,family FROM ext WHERE ext=?1", [ext], |r| {
                                Ok((r.get(0)?, r.get(1)?))
                            })?;
                        if family != o.family {
                            return Err(Error::Invalid("conflicting extension family"));
                        }
                        extensions.insert(ext.clone(), (id, family));
                        Some(id)
                    }
                } else {
                    None
                };
                conn.prepare_cached("INSERT INTO file(id,dir_id,name,name_utf16,ext_id,file_id,logical,allocated,created_ft,modified_ft,changed_ft,accessed_ft,attrs,reparse_tag,flags,seen_run) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) ON CONFLICT(id) DO UPDATE SET name=excluded.name,name_utf16=excluded.name_utf16,ext_id=excluded.ext_id,file_id=excluded.file_id,logical=excluded.logical,allocated=excluded.allocated,created_ft=excluded.created_ft,modified_ft=excluded.modified_ft,changed_ft=excluded.changed_ft,accessed_ft=excluded.accessed_ft,attrs=excluded.attrs,reparse_tag=excluded.reparse_tag,flags=excluded.flags,seen_run=excluded.seen_run")?.execute(params![id,l.dir_id,o.name,o.name_utf16,ext,file_id,o.logical as i64,o.allocated.map(|v|v as i64),o.created_ft,o.modified_ft,o.changed_ft,o.accessed_ft,o.attrs,o.reparse_tag,o.flags,l.run_id])?;
            } else {
                conn.prepare_cached("INSERT INTO dir(id,grant_id,parent_id,name,name_utf16,file_id,depth,attrs,reparse_tag,flags,created_ft,modified_ft,changed_ft,state,seen_run) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'unscanned',0) ON CONFLICT(id) DO UPDATE SET parent_id=excluded.parent_id,depth=excluded.depth,name=excluded.name,name_utf16=excluded.name_utf16,file_id=excluded.file_id,attrs=excluded.attrs,reparse_tag=excluded.reparse_tag,flags=excluded.flags,created_ft=excluded.created_ft,modified_ft=excluded.modified_ft,changed_ft=excluded.changed_ft")?.execute(params![id,grant,l.dir_id,o.name,o.name_utf16,file_id,depth+1,o.attrs,o.reparse_tag,o.flags,o.created_ft,o.modified_ft,o.changed_ft])?;
            }
        }
    }
    let status_changed: bool = conn.query_row(
        "SELECT state!=?2 OR own_skipped!=?3 OR own_errors!=?4 FROM dir WHERE id=?1",
        params![l.dir_id, l.state, l.skipped as i64, l.errors as i64],
        |r| r.get(0),
    )?;
    changed |= status_changed;
    conn.execute("UPDATE dir SET state=?2,seen_run=?3,own_skipped=?4,own_errors=?5,listing_rev=listing_rev+?6,own_files=(SELECT count(*) FROM file WHERE dir_id=?1),own_logical=(SELECT coalesce(sum(logical),0) FROM file WHERE dir_id=?1),own_allocated=(SELECT coalesce(sum(allocated),0) FROM file WHERE dir_id=?1),own_alloc_unknown=(SELECT count(*) FROM file WHERE dir_id=?1 AND allocated IS NULL) WHERE id=?1",params![l.dir_id,l.state,l.run_id,l.skipped as i64,l.errors as i64,i64::from(changed)])?;
    if changed {
        conn.execute("WITH RECURSIVE ancestors(id) AS (SELECT parent_id FROM dir WHERE id=?1 UNION ALL SELECT d.parent_id FROM dir d JOIN ancestors a ON d.id=a.id WHERE d.parent_id IS NOT NULL) UPDATE dir SET listing_rev=listing_rev+1,sub_complete=0 WHERE id IN (SELECT id FROM ancestors)",[l.dir_id])?;
        conn.execute("UPDATE dir SET sub_complete=0 WHERE id=?1", [l.dir_id])?;
    }
    Ok(WriteReply::Listing {
        revision: conn.query_row("SELECT listing_rev FROM dir WHERE id=?1", [l.dir_id], |r| {
            r.get(0)
        })?,
    })
}
pub(crate) fn rollup(conn: &Connection, grant: i64, clean: bool) -> Result<()> {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS rollup_changed(id INTEGER PRIMARY KEY);DELETE FROM rollup_changed")?;
    let max: i64 = conn.query_row(
        "SELECT coalesce(max(depth),0) FROM dir WHERE grant_id=?1",
        [grant],
        |r| r.get(0),
    )?;
    for depth in (0..=max).rev() {
        conn.execute("INSERT INTO rollup_changed SELECT id FROM dir WHERE grant_id=?1 AND depth=?2 AND (sub_files!=own_files+(SELECT coalesce(sum(sub_files),0) FROM dir c WHERE c.parent_id=dir.id) OR sub_dirs!=(SELECT coalesce(sum(sub_dirs+1),0) FROM dir c WHERE c.parent_id=dir.id) OR sub_logical!=own_logical+(SELECT coalesce(sum(sub_logical),0) FROM dir c WHERE c.parent_id=dir.id) OR sub_allocated!=own_allocated+(SELECT coalesce(sum(sub_allocated),0) FROM dir c WHERE c.parent_id=dir.id) OR sub_alloc_unknown!=own_alloc_unknown+(SELECT coalesce(sum(sub_alloc_unknown),0) FROM dir c WHERE c.parent_id=dir.id) OR sub_complete!=(?3 AND state='complete' AND own_errors=0 AND own_skipped=0 AND NOT EXISTS(SELECT 1 FROM dir c WHERE c.parent_id=dir.id AND sub_complete=0)))",params![grant,depth,clean])?;
        conn.execute("UPDATE dir SET sub_files=own_files+(SELECT coalesce(sum(sub_files),0) FROM dir c WHERE c.parent_id=dir.id),sub_dirs=(SELECT coalesce(sum(sub_dirs+1),0) FROM dir c WHERE c.parent_id=dir.id),sub_logical=own_logical+(SELECT coalesce(sum(sub_logical),0) FROM dir c WHERE c.parent_id=dir.id),sub_allocated=own_allocated+(SELECT coalesce(sum(sub_allocated),0) FROM dir c WHERE c.parent_id=dir.id),sub_alloc_unknown=own_alloc_unknown+(SELECT coalesce(sum(sub_alloc_unknown),0) FROM dir c WHERE c.parent_id=dir.id),sub_skipped=own_skipped+(SELECT coalesce(sum(sub_skipped),0) FROM dir c WHERE c.parent_id=dir.id),sub_errors=own_errors+(SELECT coalesce(sum(sub_errors),0) FROM dir c WHERE c.parent_id=dir.id),sub_newest_ft=(SELECT max(ft) FROM (SELECT modified_ft ft FROM file WHERE dir_id=dir.id UNION ALL SELECT sub_newest_ft FROM dir c WHERE c.parent_id=dir.id)),sub_complete=(?3 AND state='complete' AND own_errors=0 AND own_skipped=0 AND NOT EXISTS(SELECT 1 FROM dir c WHERE c.parent_id=dir.id AND sub_complete=0)) WHERE grant_id=?1 AND depth=?2",params![grant,depth,clean])?;
    }
    conn.execute("WITH RECURSIVE ancestors(id) AS (SELECT id FROM rollup_changed UNION SELECT d.parent_id FROM dir d JOIN ancestors a ON d.id=a.id WHERE d.parent_id IS NOT NULL) UPDATE dir SET listing_rev=listing_rev+1 WHERE id IN (SELECT id FROM ancestors)",[])?;
    Ok(())
}
