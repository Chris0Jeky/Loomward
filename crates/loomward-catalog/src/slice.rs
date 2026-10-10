use crate::{
    model::timestamp,
    query::{direct, get, live, revisions, threads, Raw},
    *,
};
use rusqlite::Connection;
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
};

struct Node {
    row: Raw,
    kind: String,
    depth: usize,
    live: bool,
    children: Vec<usize>,
    folded: Option<u64>,
}
fn virtual_row(key: NodeKey, name: String) -> Raw {
    Raw {
        key,
        name,
        totals: Totals {
            complete: true,
            ..Totals::default()
        },
        modified: None,
        attrs: 0,
        flags: 0,
        extension: None,
        family: None,
        state: "complete".into(),
        parent: None,
        grant: 0,
        file_id: None,
        created: None,
        changed: None,
        accessed: None,
        born_run: 0,
        valid: true,
    }
}
fn sum_into(parent: &mut Raw, child: &Raw) -> Result<()> {
    parent.totals.logical = add(parent.totals.logical, child.totals.logical)?;
    parent.totals.allocated = add(parent.totals.allocated, child.totals.allocated)?;
    parent.totals.files = add(parent.totals.files, child.totals.files)?;
    parent.totals.dirs = add(parent.totals.dirs, child.totals.dirs)?;
    parent.totals.allocation_unknown = add(
        parent.totals.allocation_unknown,
        child.totals.allocation_unknown,
    )?;
    parent.totals.complete &= child.totals.complete;
    Ok(())
}
fn kind(row: &Raw) -> String {
    match row.key {
        NodeKey::Atlas => "atlas",
        NodeKey::Volume(_) => "volume",
        NodeKey::Dir(_) if row.parent.is_none() => "root",
        NodeKey::Dir(_) => "dir",
        NodeKey::File(_) => "file",
        NodeKey::Other(_) => "other",
    }
    .into()
}
fn expandable(row: &Raw) -> bool {
    !matches!(row.key, NodeKey::File(_) | NodeKey::Other(_))
}

impl Reader {
    pub fn slice(&mut self, req: &SliceRequest) -> Result<TreeSlice> {
        self.slice_with_overlay(req, &HashMap::new())
    }
    /// The engine supplies its live arena; stored observations remain usable without it.
    pub fn slice_with_overlay(
        &mut self,
        req: &SliceRequest,
        overlay: &HashMap<i64, Totals>,
    ) -> Result<TreeSlice> {
        if !(1..=8).contains(&req.depth)
            || !(16..=6000).contains(&req.max_nodes)
            || !req.min_share.is_finite()
            || !(0.0..=0.1).contains(&req.min_share)
        {
            return Err(Error::Invalid("slice bounds"));
        }
        let (slice, clamps) = self.read(|conn| slice(conn, req, overlay))?;
        self.live_clamp_count = self.live_clamp_count.saturating_add(clamps);
        Ok(slice)
    }
}
fn slice(
    conn: &Connection,
    req: &SliceRequest,
    overlay: &HashMap<i64, Totals>,
) -> Result<(TreeSlice, u64)> {
    let mut clamps = 0;
    let mut virtual_children: HashMap<NodeKey, Vec<Raw>> = HashMap::new();
    let mut roots = Vec::new();
    let mut atlas = virtual_row(NodeKey::Atlas, "Atlas".into());
    if matches!(req.anchor, NodeKey::Atlas | NodeKey::Volume(_)) {
        let mut stmt=conn.prepare("SELECT d.id,g.volume_id FROM dir d JOIN root_grant g ON g.id=d.root_id WHERE d.parent_id IS NULL AND g.state='active' ORDER BY g.id LIMIT 65")?;
        let root_ids = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if root_ids.len() > 64 {
            return Err(Error::Invalid("root limit"));
        }
        for (id, volume) in root_ids {
            let mut root = get(conn, NodeKey::Dir(id))?;
            if let Some(t) = overlay.get(&id) {
                root.totals = t.clone();
            }
            let key = volume.map(NodeKey::Volume).unwrap_or(NodeKey::Atlas);
            virtual_children.entry(key).or_default().push(root.clone());
            roots.push(root);
        }
        let mut volumes = Vec::new();
        for (key, children) in &virtual_children {
            if let NodeKey::Volume(id) = key {
                let name: String =
                    conn.query_row("SELECT display_name FROM volume WHERE id=?1", [id], |r| {
                        r.get(0)
                    })?;
                let mut volume = virtual_row(*key, name);
                for child in children {
                    sum_into(&mut volume, child)?;
                }
                volumes.push(volume);
            }
        }
        let orphan = virtual_children.remove(&NodeKey::Atlas).unwrap_or_default();
        volumes.extend(orphan);
        for volume in &volumes {
            sum_into(&mut atlas, volume)?;
        }
        virtual_children.insert(NodeKey::Atlas, volumes);
    }
    let mut anchor = match req.anchor {
        NodeKey::Atlas => atlas,
        NodeKey::Volume(id) => virtual_children
            .get(&NodeKey::Atlas)
            .and_then(|v| v.iter().find(|r| r.key == NodeKey::Volume(id)))
            .cloned()
            .ok_or(Error::NotFound)?,
        key => get(conn, key)?,
    };
    if let NodeKey::Dir(id) = anchor.key {
        if let Some(t) = overlay.get(&id) {
            anchor.totals = t.clone();
        }
    }
    let root_revs = if anchor.grant != 0 {
        revisions(conn, Some(anchor.grant))?
    } else {
        let selected: Vec<i64> = if let NodeKey::Volume(_) = req.anchor {
            virtual_children
                .get(&req.anchor)
                .into_iter()
                .flatten()
                .map(|r| r.grant)
                .collect()
        } else {
            roots.iter().map(|r| r.grant).collect()
        };
        revisions(conn, None)?
            .into_iter()
            .filter(|(g, _)| selected.contains(g))
            .collect()
    };
    let mut live_grants = HashMap::new();
    for (g, _) in &root_revs {
        live_grants.insert(*g, live(conn, *g)?);
    }
    let any_live = live_grants.values().any(|v| *v);
    if any_live {
        for (g, is_live) in &live_grants {
            if *is_live {
                let mut stmt = conn.prepare(
                    "SELECT id FROM dir WHERE root_id=?1 AND listing_state!='absent_pending'",
                )?;
                let ids = stmt
                    .query_map([g], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if ids.iter().any(|id| !overlay.contains_key(id)) {
                    return Err(Error::RepairRequired);
                }
            }
        }
        for children in virtual_children.values_mut() {
            for child in children {
                if let NodeKey::Dir(id) = child.key {
                    if let Some(t) = overlay.get(&id) {
                        child.totals = t.clone();
                    }
                }
            }
        }
    } else if !anchor.valid || roots.iter().any(|r| !r.valid) {
        return Err(Error::RepairRequired);
    }
    let anchor_size = anchor.size(req.basis);
    let complete = anchor.totals.complete && !any_live;
    let mut nodes = vec![Node {
        kind: kind(&anchor),
        row: anchor,
        depth: 0,
        live: any_live,
        children: vec![],
        folded: None,
    }];
    let mut heap = BinaryHeap::new();
    if expandable(&nodes[0].row) {
        heap.push((anchor_size, Reverse(0)));
    }
    let mut truncated = false;
    while let Some((_, Reverse(index))) = heap.pop() {
        if nodes[index].depth >= req.depth {
            truncated |= nodes[index].row.totals.files > 0 || nodes[index].row.totals.dirs > 0;
            continue;
        }
        let remaining = req.max_nodes - nodes.len();
        if remaining < 2 {
            truncated = true;
            continue;
        }
        let parent = &nodes[index];
        let mut children = match parent.row.key {
            NodeKey::Dir(id) => direct(
                conn,
                id,
                req.basis,
                remaining,
                req.include_files,
                parent.live,
            )?,
            key => virtual_children.get(&key).cloned().unwrap_or_default(),
        };
        for child in &mut children {
            if let NodeKey::Dir(id) = child.key {
                if let Some(t) = overlay.get(&id) {
                    child.totals = t.clone();
                }
            }
        }
        children.sort_by(|a, b| {
            b.size(req.basis)
                .cmp(&a.size(req.basis))
                .then_with(|| a.key.reference().cmp(&b.key.reference()))
        });
        let known_count = if matches!(parent.row.key, NodeKey::Dir(_)) {
            if parent.row.totals.dirs == 0 {
                Some(parent.row.totals.files)
            } else {
                None
            }
        } else {
            Some(children.len() as u64)
        };
        if !parent.live && children.iter().any(|r| !r.valid) {
            return Err(Error::RepairRequired);
        }
        children.retain(|r| r.size(req.basis) as f64 >= req.min_share * anchor_size as f64);
        // Reserve a slot for other before spending the bounded query result.
        children.truncate(remaining - 1);
        let parent_depth = parent.depth;
        let parent_live = parent.live;
        let parent_row = parent.row.clone();
        let mut logical = 0_u64;
        let mut allocated = 0_u64;
        let mut files = 0_u64;
        let mut dirs = 0_u64;
        let mut unknown = 0_u64;
        for child in children {
            logical = add(logical, child.totals.logical)?;
            allocated = add(allocated, child.totals.allocated)?;
            files = add(files, child.totals.files)?;
            unknown = add(unknown, child.totals.allocation_unknown)?;
            dirs = add(
                dirs,
                add(
                    child.totals.dirs,
                    u64::from(
                        matches!(child.key, NodeKey::Dir(_))
                            && matches!(parent_row.key, NodeKey::Dir(_)),
                    ),
                )?,
            )?;
            let child_live = live_grants
                .get(&child.grant)
                .copied()
                .unwrap_or(parent_live);
            let i = nodes.len();
            if expandable(&child) {
                heap.push((child.size(req.basis), Reverse(i)));
            }
            nodes.push(Node {
                kind: kind(&child),
                row: child,
                depth: parent_depth + 1,
                live: child_live,
                children: vec![],
                folded: None,
            });
            nodes[index].children.push(i);
        }
        let visible = nodes[index].children.len() as u64;
        let omitted = known_count.map(|n| n.saturating_sub(visible));
        let remainder = Totals {
            logical: difference(parent_row.totals.logical, logical, parent_live, &mut clamps)?,
            allocated: difference(
                parent_row.totals.allocated,
                allocated,
                parent_live,
                &mut clamps,
            )?,
            files: difference(parent_row.totals.files, files, parent_live, &mut clamps)?,
            dirs: difference(parent_row.totals.dirs, dirs, parent_live, &mut clamps)?,
            allocation_unknown: difference(
                parent_row.totals.allocation_unknown,
                unknown,
                parent_live,
                &mut clamps,
            )?,
            complete: parent_row.totals.complete,
            ..Totals::default()
        };
        if omitted.is_some_and(|n| n > 0)
            || remainder.logical > 0
            || remainder.files > 0
            || remainder.dirs > 0
            || remainder.allocation_unknown > 0
        {
            truncated = true;
            let mut other = virtual_row(NodeKey::Other(index as i64), "Other".into());
            other.totals = remainder;
            other.grant = parent_row.grant;
            other.state = parent_row.state;
            let i = nodes.len();
            nodes.push(Node {
                row: other,
                kind: "other".into(),
                depth: parent_depth + 1,
                live: parent_live,
                children: vec![],
                folded: omitted,
            });
            nodes[index].children.push(i);
        }
    }
    let mut output = Vec::new();
    flatten(conn, &nodes, 0, None, req.basis, &mut output)?;
    Ok((
        TreeSlice {
            anchor_node_id: req.anchor.reference(),
            basis: req.basis,
            root_generations: root_revs
                .into_iter()
                .map(|(id, rev)| RootGeneration {
                    root_id: format!("rt_{id}"),
                    generation: rev.map(|rev| rev.to_string()),
                })
                .collect(),
            complete,
            live: any_live,
            aggregate_state: if any_live {
                "provisional_live"
            } else {
                "consistent"
            }
            .into(),
            ordering: if any_live {
                "approximate_live"
            } else {
                "exact"
            }
            .into(),
            truncated,
            nodes: output,
        },
        clamps,
    ))
}
fn flatten(
    conn: &Connection,
    nodes: &[Node],
    i: usize,
    parent: Option<usize>,
    basis: Basis,
    out: &mut Vec<SliceNode>,
) -> Result<()> {
    let n = &nodes[i];
    let r = &n.row;
    let index = out.len();
    let node_threads = if r.grant != 0 {
        threads(conn, r.grant, &r.coverage(), r.flags)?
    } else {
        Threads {
            meaning: MeaningThread {
                state: "pending".into(),
                label: None,
                share: None,
                source: None,
                collection_ids: vec![],
            },
            residency: ResidencyThread {
                volume_id: match r.key {
                    NodeKey::Volume(id) => Some(format!("vo_{id}")),
                    _ => None,
                },
                tier: None,
                tier_basis: "unknown".into(),
            },
            permission: PermissionThread {
                state: if r.totals.complete {
                    "granted"
                } else {
                    "partial"
                }
                .into(),
                reason: Some("mixed_children".into()),
            },
        }
    };
    let child_count = if matches!(r.key, NodeKey::File(_) | NodeKey::Other(_)) {
        Some(0)
    } else if !n.children.is_empty() {
        None
    } else if r.totals.dirs == 0 {
        Some(r.totals.files)
    } else {
        None
    };
    out.push(SliceNode {
        node_id: r.key.reference(),
        parent,
        kind: n.kind.clone(),
        name: r.name.clone(),
        depth: n.depth,
        size_bytes: r.size(basis).to_string(),
        size_unknown_files: r.totals.allocation_unknown,
        logical_bytes: r.totals.logical.to_string(),
        allocated_bytes: (r.totals.allocation_unknown == 0).then(|| r.totals.allocated.to_string()),
        files: r.totals.files,
        dirs: r.totals.dirs,
        child_count,
        folded_count: n.folded,
        coverage: if r.grant == 0 && !r.totals.complete {
            "partial".into()
        } else {
            r.coverage()
        },
        live: n.live,
        ext_family: r.family.clone(),
        modified_at: timestamp(r.modified),
        threads: node_threads,
    });
    for child in &n.children {
        flatten(conn, nodes, *child, Some(index), basis, out)?;
    }
    Ok(())
}

fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or(Error::Invalid("aggregate byte overflow"))
}
fn difference(parent: u64, children: u64, live: bool, clamps: &mut u64) -> Result<u64> {
    match parent.checked_sub(children) {
        Some(value) => Ok(value),
        None if live => {
            *clamps += 1;
            Ok(0)
        }
        None => Err(Error::Invalid("inconsistent committed aggregate")),
    }
}
