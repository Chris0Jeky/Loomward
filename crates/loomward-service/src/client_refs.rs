//! Private, non-serializable references for in-process client hosts across idle service opens.
use crate::{catalog_error, db, fail, sql_error, ClientViewStamp, Service};
use loomward_catalog::NodeKey;
use loomward_protocol::{ErrorBody, ErrorCode};
use rusqlite::OptionalExtension;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Node(NodeKey, i64),
    Root(i64),
    Volume(i64),
}

/// Only a service can mint this handle. It is not an effect grant or a wire/persistent format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientReference {
    stamp: ClientViewStamp,
    target: Target,
}

/// Fresh session IDs for the host's next normal client call; none belongs in model output.
pub struct ClientReferenceIds {
    pub node_id: Option<String>,
    pub root_id: Option<String>,
    /// Volume identity is absent unless the live grant discloses `volumes`.
    pub volume_key: Option<String>,
}

/// The catalogue advances its revision on open, including a no-op open. Only discount that
/// exact increment, with unchanged instance and root generations (never recovery or a scan).
pub(crate) struct OpenWitness {
    revision: i64,
    instance: String,
    generations: String,
}
impl OpenWitness {
    pub(crate) fn read(dir: &std::path::Path) -> Option<Self> {
        let db = rusqlite::Connection::open_with_flags(
            dir.join("catalog.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        db.query_row("SELECT catalog_rev,(SELECT value FROM meta WHERE key='instance_id'),(SELECT json_group_array(json_array(id,generation,state,active_run)) FROM (SELECT id,generation,state,active_run FROM root ORDER BY id)) FROM revision",[],|r| Ok(Self {revision:r.get(0)?,instance:r.get(1)?,generations:r.get(2)?})).ok()
    }
}

impl Service {
    /// A host may carry references over its own no-op open only, not an intervening owner open.
    pub fn resume_client_view(
        &self,
        client: &str,
        previous: &ClientViewStamp,
    ) -> Result<ClientViewStamp, ErrorBody> {
        let current = self.client_view_stamp(client)?;
        let same = self
            .inner
            .client_open
            .as_ref()
            .is_some_and(|(before, after)| {
                before.revision == previous.catalog_rev
                    && after.revision == current.catalog_rev
                    && before.revision.checked_add(1) == Some(after.revision)
                    && before.instance == after.instance
                    && before.generations == after.generations
                    && previous.epoch == current.epoch
                    && previous.instance == current.instance
                    && previous.roots == current.roots
            });
        if same {
            Ok(current)
        } else {
            Err(fail(
                ErrorCode::NotFound,
                "unknown reference; call find again",
            ))
        }
    }

    /// Refresh the private handle after the same proven no-op open; incarnation and scope still bind.
    pub fn resume_client_reference(
        &self,
        client: &str,
        held: &ClientReference,
    ) -> Result<ClientReference, ErrorBody> {
        let reference = ClientReference {
            stamp: self.resume_client_view(client, &held.stamp)?,
            target: held.target.clone(),
        };
        self.resolve_client_reference(client, &reference)?;
        Ok(reference)
    }

    /// Holds an already disclosed node/root/volume, checking its client scope at mint and use.
    pub fn client_reference(&self, client: &str, id: &str) -> Result<ClientReference, ErrorBody> {
        let stamp = self.client_view_stamp(client)?;
        let target = if id.starts_with("rt_") {
            Target::Root(self.inner.root_row(id)?)
        } else if id.starts_with("vo_") {
            Target::Volume(self.inner.volume_row(id)?)
        } else {
            let (key, born) = self
                .inner
                .ids
                .open(id, &self.inner.instance)
                .map_err(|_| fail(ErrorCode::NotFound, "unknown node"))?;
            Target::Node(key, born)
        };
        let reference = ClientReference { stamp, target };
        self.resolve_client_reference(client, &reference)?;
        Ok(reference)
    }

    /// Re-seals a held key only under the same live grant, catalogue instance and revision.
    pub fn resolve_client_reference(
        &self,
        client: &str,
        reference: &ClientReference,
    ) -> Result<ClientReferenceIds, ErrorBody> {
        if reference.stamp != self.client_view_stamp(client)? {
            return Err(fail(
                ErrorCode::NotFound,
                "unknown reference; call find again",
            ));
        }
        let grant = self.inner.client_grant(client)?;
        let mut node = None;
        let root = match reference.target {
            Target::Node(key, born) => {
                let mut reader = self.inner.read_reader(None)?.without_repairs();
                reader
                    .check_incarnation(key, &self.inner.instance, born)
                    .map_err(|_| fail(ErrorCode::NotFound, "unknown node"))?;
                let root = self
                    .inner
                    .node_root(key)?
                    .ok_or_else(|| fail(ErrorCode::NotFound, "unknown node"))?;
                node = Some(self.inner.ids.seal(key, born, &self.inner.instance));
                Some(root)
            }
            Target::Root(root) => {
                let dir = db::active_root_dir(&self.inner.db(), root).map_err(sql_error)?;
                if let Some(dir) = dir {
                    let mut reader = self.inner.read_reader(None)?.without_repairs();
                    let born = db::born_runs(&self.inner.db(), "dir", &[dir]).map_err(sql_error)?;
                    if let Some((_, born)) = born.first() {
                        reader
                            .check_incarnation(NodeKey::Dir(dir), &self.inner.instance, *born)
                            .map_err(catalog_error)?;
                        node = Some(self.inner.ids.seal(
                            NodeKey::Dir(dir),
                            *born,
                            &self.inner.instance,
                        ));
                    }
                }
                Some(root)
            }
            Target::Volume(_) => None,
        };
        if root.is_some_and(|root| !grant.roots.contains(&root)) {
            return Err(fail(ErrorCode::NotFound, "unknown node"));
        }
        let volume = match reference.target {
            Target::Volume(volume) => {
                if !grant.has("volumes") {
                    return Err(fail(
                        ErrorCode::PermissionDenied,
                        "this grant does not disclose volumes",
                    ));
                }
                Some(volume)
            }
            _ if grant.has("volumes") => self
                .inner
                .db()
                .query_row("SELECT volume_id FROM main.root WHERE id=?1", [root], |r| {
                    r.get::<_, Option<i64>>(0)
                })
                .optional()
                .map_err(sql_error)?
                .flatten(),
            _ => None,
        };
        let volume_key = volume
            .map(|volume| {
                self.inner
                    .db()
                    .query_row(
                        "SELECT volume_key FROM main.volume WHERE id=?1",
                        [volume],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(sql_error)
            })
            .transpose()?
            .flatten();
        if matches!(reference.target, Target::Volume(_)) && volume_key.is_none() {
            return Err(fail(ErrorCode::NotFound, "unknown volume"));
        }
        if reference.stamp != self.client_view_stamp(client)? {
            return Err(fail(
                ErrorCode::NotFound,
                "unknown reference; call find again",
            ));
        }
        Ok(ClientReferenceIds {
            node_id: node,
            root_id: root.map(|root| self.inner.root_id(root)),
            volume_key,
        })
    }

    /// Composition hosts must ask the service for required fields, never infer them from nulls.
    pub fn client_archive_fields(&self, client: &str) -> Result<(), ErrorBody> {
        let grant = self.inner.client_grant(client)?;
        for (field, message) in [
            ("size", "this grant does not disclose sizes"),
            ("volumes", "this grant does not disclose volumes"),
        ] {
            if !grant.has(field) {
                return Err(fail(ErrorCode::PermissionDenied, message));
            }
        }
        Ok(())
    }
}
