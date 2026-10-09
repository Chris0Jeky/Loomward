//! Local SQLite observations. References here are internal, never authority for effects.
use std::path::Path;
mod db;
mod model;
mod query;
mod slice;
mod writer;
pub use model::*;
pub use query::Reader;
pub use writer::{Receipt, Writer};

pub const APPLICATION_ID: i64 = 0x4c4d5752;
pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug)]
pub enum Error {
    Sql(rusqlite::Error),
    Io(std::io::Error),
    Invalid(&'static str),
    ForeignDatabase,
    NewerDatabase,
    CorruptDatabase,
    DatasetMismatch,
    NotFound,
    StaleGeneration,
    Closed,
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
pub struct Catalog {
    dir: std::path::PathBuf,
    writer: Writer,
}
impl Catalog {
    pub fn open(dir: &Path, dataset: &str) -> Result<Self> {
        db::local_path(dir)?;
        if !matches!(dataset, "synthetic" | "personal") {
            return Err(Error::Invalid("dataset class"));
        }
        std::fs::create_dir_all(dir)?;
        let dir = dir.canonicalize()?;
        db::local_path(&dir)?;
        let connection = db::open_pair(&dir, dataset)?;
        let writer = Writer::start(connection)?;
        Ok(Self { dir, writer })
    }
    pub fn writer(&self) -> &Writer {
        &self.writer
    }
    pub fn reader(&self) -> Result<Reader> {
        Reader::open(&self.dir)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
