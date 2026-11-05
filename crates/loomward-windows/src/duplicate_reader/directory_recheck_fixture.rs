//! Root-scoped final identity-query hooks for disposable native fixtures only.
//! `QueryReturned` holds the reader after the OS query returns; it never simulates cancellation
//! of a still-pending synchronous kernel call. Callbacks run outside the hook registry lock.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    BeforeLoop,
    QueryReturned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Continue,
    ExpireBudget,
}

type Hook = Arc<dyn Fn(Point, &Path) -> Action + Send + Sync>;

fn hooks() -> &'static Mutex<BTreeMap<PathBuf, Hook>> {
    static HOOKS: OnceLock<Mutex<BTreeMap<PathBuf, Hook>>> = OnceLock::new();
    HOOKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub struct Guard {
    root: PathBuf,
}

/// Install one hook for this exact synthetic root; other parallel fixture roots are unaffected.
pub fn install(
    root: &Path,
    hook: impl Fn(Point, &Path) -> Action + Send + Sync + 'static,
) -> Guard {
    let root = root.to_path_buf();
    let mut hooks = hooks().lock().unwrap();
    assert!(
        !hooks.contains_key(&root),
        "root already has an identity-query hook"
    );
    hooks.insert(root.clone(), Arc::new(hook));
    Guard { root }
}

impl Drop for Guard {
    fn drop(&mut self) {
        hooks()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.root);
    }
}

pub(crate) fn fire(root: &Path, point: Point, relative: &Path) -> Action {
    let hook = hooks().lock().unwrap().get(root).cloned();
    hook.map_or(Action::Continue, |hook| hook(point, relative))
}
