//! docs/47 lane C6 (LW-041 #47): the fault and crash matrix for the C5 executor, on synthetic lab roots.
//!
//! A child process (this test binary re-run on `crash_child`) drives one step of an operation: a run,
//! the owner's abandon after recovery, or recovery alone. The `crash` kill points in `exec.rs` count
//! every boundary (each journal record before and after it commits, each create, each 1 MiB source
//! chunk, each verification, the rename step, each confirmation read). A clean child enumerates them;
//! then one child per boundary N is terminated at exactly the Nth (`TerminateProcess`, no unwinding),
//! and this process reopens the journal, runs recovery, then the owner's Abandon, and checks the oracle
//! after each: the source byte- and metadata-identical (snapshot plus the C1 verifier), no receipt says
//! committed unless C1 finds the published tree equal to the source, the journal replays, nothing new
//! appears, nothing is removed with removal off (Q1 default), only journaled identities are removed
//! with it on, and nothing outside the lab roots changes. There is no resume in C5 (a stopped run
//! exits by Abandon only), so the matrix is crash point x {recover on open, abandon after recover}.
//!
//! `cargo test -p loomward-windows --features copy-primitives crash_tests` kills each scenario once
//! per crash-point class plus a few random kills; `LOOMWARD_CRASH_SWEEP=full` kills at every boundary
//! of every scenario under both Q1 policies and adds 240 random kills (random tree x random scenario x
//! random boundary, seed `LOOMWARD_CRASH_SEED`, default below). A kill keeps the OS cache: this proves
//! crash consistency, not power-loss durability (G5).
use super::tests::{c1, set_attributes, source_snapshot, DenyCreate, Lab};
use super::*;
use loomward_operations::{tree, Entry};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    process::Command,
};

const CHILD: &str = "effects::exec::crash_tests::crash_child";
const SEED: u64 = 0x2026_1017_00C6;
const RANDOM_KILLS_FULL: usize = 240;
const KILLS_PER_TREE: usize = 5;

fn full() -> bool {
    std::env::var("LOOMWARD_CRASH_SWEEP").is_ok_and(|v| v == "full")
}

/// Hosted runners are elevated and the copy primitives refuse elevation: prove the refusal, then skip.
fn lab() -> Option<Lab> {
    if enumerate::running_elevated() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            identity::observe(temp.path()).is_err(),
            "an elevated token must be refused before any identity is observed"
        );
        eprintln!("skipped: elevated token; copy primitives refuse elevation (invariant 2)");
        return None;
    }
    Lab::new()
}

// ---- the child ------------------------------------------------------------------------------------

/// `LOOMWARD_COPY_CHILD=<action>|<removal 0/1>|<op>|<lab dir>`; absent in an ordinary test run.
#[test]
fn crash_child() {
    let Ok(spec) = std::env::var("LOOMWARD_COPY_CHILD") else {
        return;
    };
    let parts: Vec<&str> = spec.split('|').collect();
    let (action, removal, op, dir) = (parts[0], parts[1] == "1", parts[2], Path::new(parts[3]));
    let op: u64 = op.parse().unwrap();
    let store = Mutex::new(Store::open(&dir.join("state/state.db")).unwrap());
    let control = Control::new(removal);
    match action {
        "run" => {
            run(&store, op, &control).unwrap();
        }
        "recover" => {
            recover(&store, op, &control).unwrap();
        }
        "abandon" => {
            let r = recover(&store, op, &control).unwrap();
            if !r.state.is_terminal() {
                let plan: Plan =
                    serde_json::from_str(&store.lock().unwrap().plan(op).unwrap().unwrap())
                        .unwrap();
                cancel(&store, op, &plan, &control).unwrap();
            }
        }
        other => panic!("unknown child action {other}"),
    }
}

// ---- scenarios and trees --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// The child runs a fresh operation.
    Run,
    /// The destination denies folder creation: the run is refused at `preparing`.
    Refused,
    /// A run stopped after its first verified batch; the child recovers, then the owner abandons.
    AbandonVerified,
    /// A run stopped with its publish intent journaled; the child recovers, then the owner abandons.
    AbandonPublish,
    /// A run stopped between the rename and `committed`; the child recovers (post-publish confirmation).
    Confirm,
}

impl Scenario {
    const ALL: [Scenario; 5] = [
        Scenario::Run,
        Scenario::Refused,
        Scenario::AbandonVerified,
        Scenario::AbandonPublish,
        Scenario::Confirm,
    ];
    fn action(self) -> &'static str {
        match self {
            Scenario::Run | Scenario::Refused => "run",
            Scenario::AbandonVerified | Scenario::AbandonPublish => "abandon",
            Scenario::Confirm => "recover",
        }
    }
    /// The phase-1 stop (in this process) that leaves the operation where the child picks it up.
    fn stop(self) -> Option<fn(Point) -> bool> {
        match self {
            Scenario::Run | Scenario::Refused => None,
            Scenario::AbandonVerified => Some(|p| matches!(p, Point::Verified(_))),
            Scenario::AbandonPublish => Some(|p| p == Point::BeforePublish),
            Scenario::Confirm => Some(|p| p == Point::AfterPublish),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Case {
    scenario: Scenario,
    removal: bool,
    /// None: the fixed matrix tree; Some(seed): a generated tree.
    tree: Option<u64>,
}

fn pattern(size: usize, salt: u64) -> Vec<u8> {
    (0..size)
        .map(|i| ((i as u64).wrapping_mul(31).wrapping_add(salt) % 251) as u8)
        .collect()
}

/// The matrix tree: a 3 MiB + 1 file (writes at four chunk offsets), sector and odd sizes, an empty
/// file, a named stream, read-only and hidden files, a nested folder and an empty folder.
fn fixed_tree(top: &Path) {
    fs::create_dir(top).unwrap();
    for (name, size) in [
        ("big.bin", 3 * 1024 * 1024 + 1),
        ("zero.bin", 0),
        ("one.bin", 1),
        ("odd.bin", 4097),
    ] {
        fs::write(top.join(name), pattern(size, 7)).unwrap();
    }
    fs::write(top.join("streamed.txt"), b"default stream").unwrap();
    fs::write(top.join("streamed.txt:note"), b"named stream").unwrap();
    fs::write(top.join("readonly.txt"), b"read only").unwrap();
    set_attributes(&top.join("readonly.txt"), FILE_ATTRIBUTE_READONLY);
    fs::write(top.join("hidden.txt"), b"hidden").unwrap();
    set_attributes(&top.join("hidden.txt"), FILE_ATTRIBUTE_HIDDEN);
    fs::create_dir(top.join("sub")).unwrap();
    fs::write(top.join("sub").join("leaf.bin"), pattern(512, 3)).unwrap();
    fs::create_dir(top.join("empty")).unwrap();
}

/// SplitMix64: a deterministic generator with no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A random tree from `seed`: 1 to 8 files in up to 3 nested folders, sizes at sector and chunk edges,
/// some read-only, hidden, with a named stream or a Unicode name, sometimes an empty folder.
fn random_tree(top: &Path, seed: u64) {
    let mut rng = Rng(seed);
    fs::create_dir(top).unwrap();
    let mut dirs = vec![top.to_path_buf()];
    for d in 0..rng.below(4) {
        let parent = dirs[rng.below(dirs.len() as u64) as usize].clone();
        let dir = parent.join(format!("dir{d}"));
        fs::create_dir(&dir).unwrap();
        dirs.push(dir);
    }
    const SIZES: [usize; 9] = [0, 1, 511, 512, 4095, 4096, 4097, 65536, 1024 * 1024 + 1];
    for f in 0..1 + rng.below(8) {
        let dir = &dirs[rng.below(dirs.len() as u64) as usize];
        let size = match rng.below(10) {
            9 => rng.below(300_000) as usize,
            k => SIZES[k as usize % SIZES.len()],
        };
        let name = if rng.below(6) == 0 {
            format!("f\u{e9}{f}.bin")
        } else {
            format!("f{f}.bin")
        };
        let path = dir.join(name);
        fs::write(&path, pattern(size, seed ^ f)).unwrap();
        if rng.below(5) == 0 {
            let mut stream = path.clone().into_os_string();
            stream.push(":s");
            fs::write(stream, pattern(1 + rng.below(2000) as usize, f)).unwrap();
        }
        match rng.below(6) {
            0 => set_attributes(&path, FILE_ATTRIBUTE_READONLY),
            1 => set_attributes(&path, FILE_ATTRIBUTE_HIDDEN),
            _ => {}
        }
    }
    if rng.below(3) == 0 {
        fs::create_dir(top.join("empty")).unwrap();
    }
}

// ---- one prepared operation and its oracle ---------------------------------------------------------

/// Relative path under the destination parent to file ID: what was on disk at one moment.
type Snap = BTreeMap<String, [u8; 16]>;

fn dest_snapshot(root: &Path) -> Snap {
    let mut out = Snap::new();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let name = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type().unwrap();
            if kind.is_dir() && !kind.is_symlink() {
                pending.push((entry.path(), name.clone()));
            }
            out.insert(
                name,
                identity::observe(&entry.path()).unwrap().object.file_id,
            );
        }
    }
    out
}

const EXISTING: &str = "existing.txt";

struct Prepared {
    lab: Lab,
    op: u64,
    case: Case,
    before: Vec<(PathBuf, identity::ObservedIdentity, Vec<u8>)>,
    before_c1: tree::Tree,
    neighbour: Vec<(PathBuf, identity::ObservedIdentity, Vec<u8>)>,
    final_name: String,
    deny: Option<DenyCreate>,
}

impl Prepared {
    /// Tree, approval, start and the phase-1 stop for the scenario; None when elevated (skipped).
    fn new(case: Case) -> Option<Prepared> {
        let lab = lab()?;
        let top = lab.src().join("top");
        match case.tree {
            None => fixed_tree(&top),
            Some(seed) => random_tree(&top, seed),
        }
        // A pre-existing entry in the destination parent and a folder outside both lab roots.
        fs::write(lab.dest().join(EXISTING), b"pre-existing").unwrap();
        let outside = lab.tmp.path().join("neighbour");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"not a lab root").unwrap();
        let before = source_snapshot(&lab.src());
        let before_c1 = c1(&top);
        let neighbour = source_snapshot(&outside);
        let op = lab.start(&["top"]);
        let final_name = lab.plan(op).manifest.destination.folder_name;
        if let Some(at) = case.scenario.stop() {
            let stop = Control::new(case.removal).with_hook(move |p, _| {
                if at(p) {
                    Flow::Crash
                } else {
                    Flow::Continue
                }
            });
            assert!(matches!(run(&lab.store, op, &stop), Err(Fault::Crashed)));
        }
        let deny = (case.scenario == Scenario::Refused).then(|| DenyCreate::new(&lab.dest()));
        Some(Prepared {
            lab,
            op,
            case,
            before,
            before_c1,
            neighbour,
            final_name,
            deny,
        })
    }

    /// Run the scenario's child step, killed at boundary `n` (None: to the end), tracing every
    /// boundary. Returns the exit code and the trace labels.
    fn child(&self, n: Option<u64>) -> (i32, Vec<String>) {
        let trace = self
            .lab
            .tmp
            .path()
            .join(format!("trace-{}.txt", n.unwrap_or(0)));
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([CHILD, "--exact", "--test-threads=1", "--nocapture"])
            .env(
                "LOOMWARD_COPY_CHILD",
                format!(
                    "{}|{}|{}|{}",
                    self.case.scenario.action(),
                    u8::from(self.case.removal),
                    self.op,
                    self.lab.tmp.path().display()
                ),
            )
            .env("LOOMWARD_COPY_CRASH_TRACE", &trace)
            .env_remove("LOOMWARD_COPY_CRASH_AT")
            .env_remove("LOOMWARD_CRASH_SWEEP");
        if let Some(n) = n {
            cmd.env("LOOMWARD_COPY_CRASH_AT", n.to_string());
        }
        let out = cmd.output().unwrap();
        let code = out.status.code().unwrap_or(-1);
        assert!(
            code == 0 || code == crash::EXIT as i32,
            "{:?} child exited {code}\n{}\n{}",
            self.case,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let labels = fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once('\t').unwrap().1.to_string())
            .collect();
        (code, labels)
    }

    /// File IDs the journal ever recorded as Loomward's: the staging folder and every created item.
    fn journaled(&self) -> BTreeSet<[u8; 16]> {
        let rows = self.lab.store.lock().unwrap().rows(self.op).unwrap();
        let mut ids = BTreeSet::new();
        for row in rows {
            match serde_json::from_str::<Entry>(&row.body).unwrap().record {
                Record::StagingCreated { id } => {
                    ids.insert(id.file_id);
                }
                Record::Identities { items } => ids.extend(items.iter().map(|(_, id)| id.file_id)),
                _ => {}
            }
        }
        ids
    }

    /// Q1: with removal off nothing that existed is gone; with it on, only journaled identities.
    fn removals(&self, ctx: &str, from: &Snap, to: &Snap) {
        let journaled = self.journaled();
        for (path, id) in from {
            if to.get(path) == Some(id) {
                continue;
            }
            assert!(
                self.case.removal,
                "{ctx}: removal off, yet {path} is gone or replaced"
            );
            assert!(
                journaled.contains(id),
                "{ctx}: {path} removed without a journaled identity"
            );
        }
    }

    /// The oracle after recovery or abandon, against the disk as the kill left it.
    fn oracle(&self, ctx: &str, at_crash: &Snap, outcome: &Outcome) -> Snap {
        let r = replay(&self.lab.store, self.op).unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
        assert_eq!(
            r.state, outcome.state,
            "{ctx}: receipt and journal disagree"
        );
        assert_eq!(
            source_snapshot(&self.lab.src()),
            self.before,
            "{ctx}: the source changed"
        );
        let report = tree::compare(&self.before_c1, &c1(&self.lab.src().join("top")));
        assert!(report.equal(), "{ctx}: C1 finds the source changed");
        let outside = self.lab.tmp.path().join("neighbour");
        assert_eq!(
            source_snapshot(&outside),
            self.neighbour,
            "{ctx}: outside changed"
        );
        assert_eq!(
            fs::read(self.lab.dest().join(EXISTING)).unwrap(),
            b"pre-existing"
        );
        let now = dest_snapshot(&self.lab.dest());
        for (path, id) in &now {
            assert_eq!(
                at_crash.get(path),
                Some(id),
                "{ctx}: {path} appeared after the kill"
            );
        }
        self.removals(ctx, at_crash, &now);
        for name in now.keys().filter(|p| !p.contains('/')) {
            assert!(
                name == EXISTING || *name == self.final_name || r.staging_tokens.contains(name),
                "{ctx}: unexpected {name} in the destination parent"
            );
        }
        if outcome.state == OpState::Committed {
            let copy = c1(&self.lab.dest().join(&self.final_name));
            let report = tree::compare(&self.before_c1, &copy);
            assert!(report.equal(), "{ctx}: FALSE COMMIT {report:#?}");
        }
        now
    }

    /// After the kill (or a fault): recovery on open, then the owner's Abandon when it is still open.
    /// Returns (recovered, abandoned) states.
    fn recover_then_abandon(&mut self, ctx: &str, at_crash: &Snap) -> (OpState, OpState) {
        self.deny = None;
        self.lab.reopen();
        let control = Control::new(self.case.removal);
        let recovered = recover(&self.lab.store, self.op, &control)
            .unwrap_or_else(|e| panic!("{ctx}: recovery {e:?}"));
        let after_recovery = self.oracle(&format!("{ctx} recover"), at_crash, &recovered);
        let abandoned = if recovered.state.is_terminal() {
            recovered.clone()
        } else {
            cancel(&self.lab.store, self.op, &self.lab.plan(self.op), &control)
                .unwrap_or_else(|e| panic!("{ctx}: abandon {e:?}"))
        };
        assert!(
            abandoned.state.is_terminal() || abandoned.state == OpState::NeedsAttention,
            "{ctx}: abandon left {abandoned:?}"
        );
        let after = self.oracle(&format!("{ctx} abandon"), &after_recovery, &abandoned);
        if abandoned.state == OpState::Cancelled {
            // Abandon leaves the parent as before, except a staging folder left for the owner (Q1
            // off) and reported leftovers that were never journaled.
            let mut allowed: BTreeSet<&str> = BTreeSet::from([EXISTING]);
            allowed.extend(abandoned.staging_left.as_deref());
            allowed.extend(abandoned.leftover_staging.iter().map(String::as_str));
            for name in after.keys().filter(|p| !p.contains('/')) {
                assert!(
                    allowed.contains(name.as_str()),
                    "{ctx}: {name} after abandon"
                );
            }
            assert_eq!(
                abandoned.staging_left.is_some(),
                !self.case.removal && replay(&self.lab.store, self.op).unwrap().staging.is_some(),
                "{ctx}: {abandoned:?}"
            );
        }
        (recovered.state, abandoned.state)
    }
}

// ---- crash-point classes --------------------------------------------------------------------------

/// docs/47 section 10's CP-01 to CP-15 (every section 5 journal boundary, mid-write at four offsets,
/// mid-verify, mid-abandon), extended with section 12's lane C2 items and the C5 records as CP-16 to
/// CP-22. CP-F marks refusal records reached by the fault scenarios. An unknown label fails the matrix.
fn class(label: &str) -> &'static str {
    match label {
        "before staging_intent" => "CP-01",
        "after staging_intent" | "staging folder made" | "before staging_created" => "CP-02",
        "after staging_created" | "before create_batch" => "CP-03",
        "after create_batch" | "created item" | "before identities" => "CP-04",
        "after identities" => "CP-05",
        "write@0" => "CP-06",
        "write@1048576" => "CP-07",
        "write@2097152" => "CP-08",
        l if l.starts_with("write@") => "CP-09",
        "metadata set" | "verify item" | "verified item" | "before verified" => "CP-10",
        "after verified"
        | "before transition:staged"
        | "after transition:staged"
        | "before publish_intent" => "CP-11",
        "after publish_intent" => "CP-12",
        "renamed" | "before committed" => "CP-13",
        "after committed" => "CP-14",
        "before transition:cancelling"
        | "after transition:cancelling"
        | "before transition:cancelled"
        | "after transition:cancelled" => "CP-15",
        "rename" => "CP-16",
        "before publish_withdrawn" | "after publish_withdrawn" => "CP-17",
        "confirm item" => "CP-18",
        "before item_removed"
        | "after item_removed"
        | "before staging_removed"
        | "after staging_removed" => "CP-19",
        "before planned" | "after planned" => "CP-20",
        "before staging_left" | "after staging_left" => "CP-21",
        "before transition:refused" | "after transition:refused" => "CP-22",
        "before item_failed"
        | "after item_failed"
        | "before transition:blocked"
        | "after transition:blocked"
        | "before transition:needs_attention"
        | "after transition:needs_attention" => "CP-F",
        other => panic!("unclassified kill point {other:?}: add it to the CP map"),
    }
}

const REQUIRED: [&str; 22] = [
    "CP-01", "CP-02", "CP-03", "CP-04", "CP-05", "CP-06", "CP-07", "CP-08", "CP-09", "CP-10",
    "CP-11", "CP-12", "CP-13", "CP-14", "CP-15", "CP-16", "CP-17", "CP-18", "CP-19", "CP-20",
    "CP-21", "CP-22",
];

#[derive(Default)]
struct Tally {
    kills: usize,
    by_class: BTreeMap<&'static str, usize>,
    outcomes: BTreeMap<String, usize>,
    commits: usize,
}

impl Tally {
    fn record(&mut self, label: &str, recovered: OpState, abandoned: OpState) {
        self.kills += 1;
        *self.by_class.entry(class(label)).or_default() += 1;
        *self
            .outcomes
            .entry(format!("{} -> {}", recovered.name(), abandoned.name()))
            .or_default() += 1;
        self.commits += usize::from(abandoned == OpState::Committed);
    }
}

/// Enumerate the case's boundaries with a clean child, checking its own result with the oracle.
fn enumerate_case(case: Case) -> Option<Vec<String>> {
    let mut p = Prepared::new(case)?;
    let pre = dest_snapshot(&p.lab.dest());
    let (code, labels) = p.child(None);
    assert_eq!(code, 0, "{case:?}: the clean child must finish");
    assert!(!labels.is_empty(), "{case:?}: no boundary reached");
    let at_end = dest_snapshot(&p.lab.dest());
    p.removals(&format!("{case:?} clean child"), &pre, &at_end);
    p.recover_then_abandon(&format!("{case:?} clean"), &at_end);
    Some(labels)
}

/// Kill the case's child at boundary `n` (1-based), then recover and abandon under the oracle.
fn kill(case: Case, n: usize, labels: &[String], tally: &mut Tally) {
    let Some(mut p) = Prepared::new(case) else {
        return;
    };
    let ctx = format!("{case:?} kill {n} at {:?}", labels[n - 1]);
    let pre = dest_snapshot(&p.lab.dest());
    let (code, trace) = p.child(Some(n as u64));
    assert_eq!(code, crash::EXIT as i32, "{ctx}: the child was not killed");
    assert_eq!(
        trace.last(),
        Some(&labels[n - 1]),
        "{ctx}: boundaries drifted"
    );
    let at_crash = dest_snapshot(&p.lab.dest());
    p.removals(&format!("{ctx} child"), &pre, &at_crash);
    let (recovered, abandoned) = p.recover_then_abandon(&ctx, &at_crash);
    tally.record(&labels[n - 1], recovered, abandoned);
}

/// Default: each scenario killed once per crash-point class it reaches. Full: every boundary of
/// every scenario under both Q1 policies.
#[test]
fn crash_matrix_every_point_recovers_and_abandons_under_the_oracle() {
    if lab().is_none() {
        return;
    }
    let mut tally = Tally::default();
    let mut lines = Vec::new();
    for scenario in Scenario::ALL {
        let policies: &[bool] = match (full(), scenario) {
            (true, _) => &[false, true],
            (false, Scenario::AbandonVerified) => &[false, true],
            (false, Scenario::AbandonPublish) => &[true],
            (false, _) => &[false],
        };
        for &removal in policies {
            let case = Case {
                scenario,
                removal,
                tree: None,
            };
            let labels = enumerate_case(case).unwrap();
            let mut seen = BTreeSet::new();
            let picks: Vec<usize> = (1..=labels.len())
                .filter(|&n| full() || seen.insert(class(&labels[n - 1])))
                .collect();
            for &n in &picks {
                kill(case, n, &labels, &mut tally);
            }
            lines.push(format!(
                "C6 matrix {scenario:?} removal={removal}: {} boundaries, {} kills",
                labels.len(),
                picks.len()
            ));
        }
    }
    report("matrix", &tally, &lines);
    for cp in REQUIRED {
        assert!(tally.by_class.contains_key(cp), "{cp} never killed");
    }
}

/// Random kills: a seeded random tree, scenario and Q1 policy per tree, then kills at random
/// boundaries of it. Default: 2 trees x 2 kills; full: 240 kills.
#[test]
fn random_kills_on_random_trees_keep_the_oracle() {
    if lab().is_none() {
        return;
    }
    let seed = std::env::var("LOOMWARD_CRASH_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SEED);
    let (target, per_tree) = if full() {
        (RANDOM_KILLS_FULL, KILLS_PER_TREE)
    } else {
        (4, 2)
    };
    let mut rng = Rng(seed);
    let mut tally = Tally::default();
    let mut trees = 0;
    const CHOICES: [Scenario; 4] = [
        Scenario::Run,
        Scenario::AbandonVerified,
        Scenario::AbandonPublish,
        Scenario::Confirm,
    ];
    while tally.kills < target {
        let case = Case {
            scenario: CHOICES[rng.below(CHOICES.len() as u64) as usize],
            removal: rng.below(2) == 1,
            tree: Some(rng.next()),
        };
        trees += 1;
        let labels = enumerate_case(case).unwrap();
        for _ in 0..per_tree.min(target - tally.kills) {
            let n = 1 + rng.below(labels.len() as u64) as usize;
            kill(case, n, &labels, &mut tally);
        }
    }
    report(
        "random",
        &tally,
        &[format!(
            "C6 random seed={seed:#x}: {trees} trees, {} kills",
            tally.kills
        )],
    );
}

fn report(name: &str, tally: &Tally, lines: &[String]) {
    for l in lines {
        println!("{l}");
    }
    println!(
        "C6 {name}: {} kills, 0 source changes, 0 false commits ({} verified commits)",
        tally.kills, tally.commits
    );
    for (cp, n) in &tally.by_class {
        println!("C6 {name} {cp}: {n}");
    }
    for (o, n) in &tally.outcomes {
        println!("C6 {name} outcome {o}: {n}");
    }
}

// ---- injected faults (live, no kill) -------------------------------------------------------------

fn fixed(removal: bool) -> Option<Prepared> {
    Prepared::new(Case {
        scenario: Scenario::Run,
        removal,
        tree: None,
    })
}

fn staging_count(p: &Prepared) -> usize {
    p.lab
        .dest_names()
        .iter()
        .filter(|n| n.starts_with("~lw-staging-"))
        .count()
}

/// Disk full at a write (the C5 seam: ERROR_DISK_FULL where the preallocation or write returns it):
/// `blocked` with `insufficient_space` on the item, staging kept, then Abandon under each Q1 policy.
#[test]
fn disk_full_at_a_write_blocks_keeps_staging_and_abandon_applies_q1() {
    for removal in [false, true] {
        let Some(mut p) = fixed(removal) else { return };
        let pre = dest_snapshot(&p.lab.dest());
        let control = Control::new(removal).with_hook(|point, _| match point {
            Point::Writing(_) => Flow::Fail(112),
            _ => Flow::Continue,
        });
        let out = run(&p.lab.store, p.op, &control).unwrap();
        assert_eq!(out.state, OpState::Blocked, "{out:?}");
        assert!(
            matches!(out.refusal, Some((Refusal::InsufficientSpace, Some(_)))),
            "{out:?}"
        );
        assert_eq!(staging_count(&p), 1, "staging kept");
        let after = dest_snapshot(&p.lab.dest());
        p.removals("disk full", &pre, &after);
        let (_, abandoned) = p.recover_then_abandon("disk full", &after);
        assert_eq!(abandoned, OpState::Cancelled);
        assert_eq!(staging_count(&p), usize::from(!removal));
    }
}

/// A source file held open by a writer (a real sharing violation on the deny-write open):
/// `source_in_use`, resumable, never published.
#[test]
fn a_held_source_is_source_in_use_and_never_published() {
    let Some(mut p) = fixed(true) else { return };
    let writer = fs::OpenOptions::new()
        .append(true)
        .open(p.lab.src().join("top/odd.bin"))
        .unwrap();
    let out = run(&p.lab.store, p.op, &Control::new(true)).unwrap();
    drop(writer);
    assert_eq!(out.state, OpState::Blocked, "{out:?}");
    assert!(
        matches!(out.refusal, Some((Refusal::SourceInUse, Some(_)))),
        "{out:?}"
    );
    assert!(!p.lab.dest().join(&p.final_name).exists());
    let after = dest_snapshot(&p.lab.dest());
    let (_, abandoned) = p.recover_then_abandon("source in use", &after);
    assert_eq!(abandoned, OpState::Cancelled);
}

/// Access denied on create, both places: the staging folder (refused at `preparing`, nothing created)
/// and an item inside staging (an inherit-only deny: `blocked`, `access_denied`, staging kept).
#[test]
fn access_denied_on_create_refuses_or_blocks_with_nothing_published() {
    let Some(mut p) = fixed(true) else { return };
    let deny = DenyCreate::new(&p.lab.dest());
    let out = run(&p.lab.store, p.op, &Control::new(true)).unwrap();
    drop(deny);
    assert_eq!(out.state, OpState::Refused, "{out:?}");
    assert_eq!(out.refusal, Some((Refusal::AccessDenied, None)));
    assert_eq!(p.lab.dest_names(), [EXISTING]);
    let after = dest_snapshot(&p.lab.dest());
    p.recover_then_abandon("staging denied", &after);

    for removal in [false, true] {
        let Some(mut p) = fixed(removal) else { return };
        let deny = DenyCreate::with(&p.lab.dest(), "(OI)(CI)(IO)(WD,AD)");
        let out = run(&p.lab.store, p.op, &Control::new(removal)).unwrap();
        assert_eq!(out.state, OpState::Blocked, "{out:?}");
        assert!(
            matches!(out.refusal, Some((Refusal::AccessDenied, Some(_)))),
            "{out:?}"
        );
        assert_eq!(staging_count(&p), 1, "staging kept");
        let after = dest_snapshot(&p.lab.dest());
        p.deny = Some(deny);
        let (_, abandoned) = p.recover_then_abandon("item denied", &after);
        assert_eq!(abandoned, OpState::Cancelled);
    }
}

/// The source changes mid-copy and the process dies; and the source changes after the rename and
/// before `committed`. Neither is ever committed; the published copy is never removed.
#[test]
fn a_source_changed_mid_copy_or_before_confirmation_is_never_committed() {
    for removal in [false, true] {
        let Some(mut p) = fixed(removal) else { return };
        let victim = p.lab.src().join("top/odd.bin");
        let control = Control::new(removal).with_hook(move |point, _| match point {
            Point::Identities(_) => {
                fs::write(&victim, pattern(4097, 99)).unwrap();
                Flow::Continue
            }
            Point::Verified(_) => Flow::Crash,
            _ => Flow::Continue,
        });
        let out = run(&p.lab.store, p.op, &control);
        // Either the changed item is refused before the stop, or the stop comes first.
        assert!(
            matches!(&out, Err(Fault::Crashed))
                || matches!(&out, Ok(o) if matches!(o.refusal, Some((Refusal::SourceChanged, _)))),
            "{out:?}"
        );
        p.before = source_snapshot(&p.lab.src());
        p.before_c1 = c1(&p.lab.src().join("top"));
        let after = dest_snapshot(&p.lab.dest());
        let (recovered, abandoned) = p.recover_then_abandon("source changed mid-copy", &after);
        assert_ne!(recovered, OpState::Committed);
        assert_ne!(abandoned, OpState::Committed);
        assert!(!p.lab.dest().join(&p.final_name).exists());
    }

    let Some(mut p) = Prepared::new(Case {
        scenario: Scenario::Confirm,
        removal: true,
        tree: None,
    }) else {
        return;
    };
    let published = p.lab.dest().join(&p.final_name).join("odd.bin");
    let copy = fs::read(&published).unwrap();
    fs::write(p.lab.src().join("top/odd.bin"), pattern(4097, 99)).unwrap();
    p.before = source_snapshot(&p.lab.src());
    p.before_c1 = c1(&p.lab.src().join("top"));
    let after = dest_snapshot(&p.lab.dest());
    let (recovered, abandoned) = p.recover_then_abandon("source changed before confirm", &after);
    assert_ne!(recovered, OpState::Committed);
    assert_eq!(abandoned, OpState::NeedsAttention);
    assert_eq!(fs::read(&published).unwrap(), copy, "published copy kept");
}
