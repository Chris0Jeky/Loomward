//! Local SQLite observations. References here are internal, never authority for effects.
//!
//! Pending reference intents commit to FULL state.db before publication. A catalogue-only
//! publication carrying intents switches main.synchronous from NORMAL to FULL before BEGIN,
//! fsyncs its WAL commit and restores NORMAL after commit or rollback. Only then does a
//! state-only FULL transaction confirm the matching publication witness and clear the journal.
//! Publications without intents keep NORMAL (ADR-V3-22); no transaction writes both files.
//! Membership readers accept the confirmed binding or a pending binding whose instance and
//! token match the published witness, keeping surviving hard links visible before confirmation.
#![forbid(unsafe_code)]
use std::path::Path;
mod db;
mod duplicates;
pub use duplicates::{DuplicateCandidate, DuplicateCandidates};
mod model;
mod query;
mod search;
mod slice;
mod writer;
pub use model::*;
pub use query::Reader;
pub use writer::{
    BytePermit, Cancellation, Receipt, Writer, WriterTimings, CHUNK_ENTRIES, QUEUE_BYTES,
};

pub const APPLICATION_ID: i64 = 0x4c4d5752;
pub const SCHEMA_VERSION: i64 = 7;

#[derive(Debug)]
pub enum Error {
    Sql(rusqlite::Error),
    /// An otherwise successful command was rolled back with its transaction.
    BatchAborted(String),
    Io(std::io::Error),
    Invalid(&'static str),
    ForeignDatabase,
    NewerDatabase,
    CorruptDatabase,
    DatasetMismatch,
    NotFound,
    StaleGeneration,
    Closed,
    Cancelled,
    RepairRequired,
    ResourceBudget,
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
/// The form of a local directory to hand SQLite (#390): `\\?\C:\x` becomes `C:\x`. SQLite's
/// Windows VFS treats any path starting `\\` as UNC and then shares one `-shm` lock handle among a
/// process's connections; its in-process accounting races when two readers end together, leaking a
/// WAL read lock until every connection to the file closes, so the log can never be cut. Paths used
/// for comparisons (grants, `--static`) keep their canonical form; only SQLite opens use this.
// ponytail: a plain path past MAX_PATH would fail to open, so such a path stays verbatim.
pub fn sqlite_dir(dir: &Path) -> std::path::PathBuf {
    let text = dir.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest)
            if rest.len() < 200
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.get(1..3) == Some(r":\") =>
        {
            rest.into()
        }
        _ => dir.into(),
    }
}
pub struct Catalog {
    dir: std::path::PathBuf,
    writer: std::sync::Arc<Writer>,
}
impl Catalog {
    /// Inspect existing stores without creating, migrating or repairing anything.
    pub fn validate_stores(dir: &Path, dataset: &str) -> Result<()> {
        db::local_path(dir)?;
        db::inspect(&dir.join("state.db"), dataset)?;
        match db::inspect(&dir.join("catalog.db"), dataset) {
            Ok(_) | Err(Error::CorruptDatabase) => Ok(()),
            Err(e) => Err(e),
        }
    }
    pub fn open(dir: &Path, dataset: &str) -> Result<Self> {
        db::local_path(dir)?;
        if !matches!(dataset, "synthetic" | "personal") {
            return Err(Error::Invalid("dataset class"));
        }
        std::fs::create_dir_all(dir)?;
        let dir = sqlite_dir(&dir.canonicalize()?);
        db::local_path(&dir)?;
        let connection = db::open_pair(&dir, dataset)?;
        let writer = std::sync::Arc::new(Writer::start(connection)?);
        Ok(Self { dir, writer })
    }
    pub fn writer(&self) -> &Writer {
        &self.writer
    }
    pub fn reader(&self) -> Result<Reader> {
        let mut reader = Reader::open(&self.dir)?;
        reader.writer = Some(self.writer.clone());
        Ok(reader)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
