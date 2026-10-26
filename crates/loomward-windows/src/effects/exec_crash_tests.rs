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
//!
//! The journal-append fault matrix (#535, bottom of the file) fails a chosen `Store::append` instead of
//! killing the process: SQLITE_FULL or SQLITE_BUSY, before the row, before the head anchor, or after the
//! commit (a lost acknowledgement), at every journal record the scenario appends, in this process.
use super::tests::{c1, set_attributes, source_snapshot, DenyCreate, Lab};
use super::*;
use loomward_operations::store::fault::{Hook, Kind as Fail, Stage};
use loomward_operations::{tree, Entry};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    process::Command,
    sync::Arc,
};

const CHILD: &str = "effects::exec::crash_tests::crash_child";
const SEED: u64 = 0x2026_1017_00C6;
const RANDOM_KILLS_FULL: usize = 240;
const KILLS_PER_TREE: usize = 5;
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

fn full() -> bool {
    std::env::var("LOOMWARD_CRASH_SWEEP").is_ok_and(|v| v == "full")
}

/// Hosted runners are elevated and the copy primitives refuse elevation: prove the refusal, then skip.
fn lab() -> Option<Lab> {
    if enumerate::skip_elevated_test() {
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
        "commit_pending_fence" => commit_pending_fence(&store, op),
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

/// The rename has returned with a fence still pending, the state covered by Journal::commit (#751).
fn commit_pending_fence(store: &Mutex<Store>, op: u64) {
    let control = Control::new(false);
    let step = control.admit().ok().unwrap();
    assert_eq!(
        fence_waiting(
            store,
            op,
            &control,
            loomward_operations::FenceCause::GrantRevoked,
            Duration::ZERO,
        )
        .unwrap(),
        Ack::PendingInFlightStep
    );
    drop(step);
    let epoch = replay(store, op).unwrap().epoch;
    Journal { store, op, epoch }
        .commit(&control)
        .map_err(fault)
        .unwrap();
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

pub(super) fn pattern(size: usize, salt: u64) -> Vec<u8> {
    (0..size)
        .map(|i| ((i as u64).wrapping_mul(31).wrapping_add(salt) % 251) as u8)
        .collect()
}

/// The matrix tree: a 3 MiB + 1 file (writes at four chunk offsets), sector and odd sizes, an empty
/// file, a named stream, read-only and hidden files, a nested folder and an empty folder.
pub(super) fn fixed_tree(top: &Path) {
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
pub(super) type Snap = BTreeMap<String, [u8; 16]>;

pub(super) fn dest_snapshot(root: &Path) -> Snap {
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

pub(super) const EXISTING: &str = "existing.txt";

/// File IDs the journal ever recorded as Loomward's: the staging folder and every created item.
fn journaled(lab: &Lab, op: u64) -> BTreeSet<[u8; 16]> {
    let rows = lab.store.lock().unwrap().rows(op).unwrap();
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
fn removals(lab: &Lab, op: u64, removal: bool, ctx: &str, from: &Snap, to: &Snap) {
    let journaled = journaled(lab, op);
    for (path, id) in from {
        if to.get(path) == Some(id) {
            continue;
        }
        assert!(
            removal,
            "{ctx}: removal off, yet {path} is gone or replaced"
        );
        assert!(
            journaled.contains(id),
            "{ctx}: {path} removed without a journaled identity"
        );
    }
}

/// C6 checks 1 and 4-6, shared with the reboot oracle's post-boot baseline.
pub(super) fn destination_oracle(
    lab: &Lab,
    op: u64,
    removal: bool,
    final_name: &str,
    ctx: &str,
    at_crash: &Snap,
    outcome: &Outcome,
) -> Snap {
    let r = replay(&lab.store, op).unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
    assert_eq!(
        r.state, outcome.state,
        "{ctx}: receipt and journal disagree"
    );
    let now = dest_snapshot(&lab.dest());
    for (path, id) in &now {
        assert_eq!(
            at_crash.get(path),
            Some(id),
            "{ctx}: {path} appeared after the kill"
        );
    }
    removals(lab, op, removal, ctx, at_crash, &now);
    for name in now.keys().filter(|p| !p.contains('/')) {
        assert!(
            name == EXISTING || name == final_name || r.staging_tokens.contains(name),
            "{ctx}: unexpected {name} in the destination parent"
        );
    }
    now
}

/// Abandon an open operation and enforce C6's terminal-state rule.
pub(super) fn abandon(
    lab: &Lab,
    op: u64,
    control: &Control,
    ctx: &str,
    recovered: &Outcome,
) -> Outcome {
    let abandoned = if recovered.state.is_terminal() {
        recovered.clone()
    } else {
        cancel(&lab.store, op, &lab.plan(op), control)
            .unwrap_or_else(|e| panic!("{ctx}: abandon {e:?}"))
    };
    assert!(
        abandoned.state.is_terminal() || abandoned.state == OpState::NeedsAttention,
        "{ctx}: abandon left {abandoned:?}"
    );
    abandoned
}

/// C6 check 7: reported staging leftovers are the only entries allowed after cancellation.
pub(super) fn cancelled_oracle(
    lab: &Lab,
    op: u64,
    removal: bool,
    ctx: &str,
    abandoned: &Outcome,
    after: &Snap,
) {
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
            !removal && replay(&lab.store, op).unwrap().staging.is_some(),
            "{ctx}: {abandoned:?}"
        );
    }
}

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
    fn removals(&self, ctx: &str, from: &Snap, to: &Snap) {
        removals(&self.lab, self.op, self.case.removal, ctx, from, to);
    }

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
        self.child_action(self.case.scenario.action(), n)
    }

    fn child_action(&self, action: &str, n: Option<u64>) -> (i32, Vec<String>) {
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
                    action,
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
        // Output goes to a file (no pipe to drain), so a hung child fails after the timeout, not hangs.
        let log = self
            .lab
            .tmp
            .path()
            .join(format!("child-{}.log", n.unwrap_or(0)));
        let file = fs::File::create(&log).unwrap();
        let mut child = cmd
            .stdout(file.try_clone().unwrap())
            .stderr(file)
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > CHILD_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{:?} child {n:?} hung past {CHILD_TIMEOUT:?}", self.case);
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let code = status.code().unwrap_or(-1);
        assert!(
            code == 0 || code == crash::EXIT as i32,
            "{:?} child exited {code}\n{}",
            self.case,
            fs::read_to_string(&log).unwrap_or_default()
        );
        let labels = fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split_once('\t').unwrap().1.to_string())
            .collect();
        (code, labels)
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
        let now = destination_oracle(
            &self.lab,
            self.op,
            self.case.removal,
            &self.final_name,
            ctx,
            at_crash,
            outcome,
        );
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
        let abandoned = abandon(&self.lab, self.op, &control, ctx, &recovered);
        let after = self.oracle(&format!("{ctx} abandon"), &after_recovery, &abandoned);
        cancelled_oracle(
            &self.lab,
            self.op,
            self.case.removal,
            ctx,
            &abandoned,
            &after,
        );
        (recovered.state, abandoned.state)
    }
}

// ---- crash-point classes --------------------------------------------------------------------------

/// docs/47 section 10's CP-01 to CP-15 (every section 5 journal boundary, mid-write at four offsets,
/// mid-verify, mid-abandon), extended with section 12's lane C2 items and the C5 records as CP-16 to
/// CP-22. CP-F marks refusal records reached by the fault scenarios; CP-Fence covers the pending
/// annotation before commit (#751). An unknown label fails the matrix.
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
        "metadata set"
        | "verify item"
        | "verified item"
        | "before verified"
        | "directory metadata set"
        | "staging metadata set" => "CP-10",
        "after verified"
        | "before transition:staged"
        | "after transition:staged"
        | "before publish_intent" => "CP-11",
        "after publish_intent" => "CP-12",
        "renamed" | "before committed" => "CP-13",
        "after committed" => "CP-14",
        "before fenced" | "after fenced" => "CP-Fence",
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

/// #751: a durable pending-fence annotation followed by process death still asks for confirmation
/// of the already published copy. The child exits without unwinding at both sides of that gap.
#[test]
fn a_crash_between_pending_fence_and_commit_keeps_confirm_and_annotation() {
    let case = Case {
        scenario: Scenario::Confirm,
        removal: false,
        tree: None,
    };
    let Some(clean) = Prepared::new(case) else {
        return;
    };
    let (code, labels) = clean.child_action("commit_pending_fence", None);
    assert_eq!(code, 0);
    assert_eq!(
        labels,
        [
            "before fenced",
            "after fenced",
            "before committed",
            "after committed"
        ],
        "every append boundary must be injectable, in durable order"
    );
    assert_eq!(class("before fenced"), "CP-Fence");
    assert_eq!(class("after fenced"), "CP-Fence");
    for label in ["after fenced", "before committed"] {
        let n = labels.iter().position(|l| l == label).unwrap() as u64 + 1;
        let mut p = Prepared::new(case).unwrap();
        let at_crash = dest_snapshot(&p.lab.dest());
        let (code, reached) = p.child_action("commit_pending_fence", Some(n));
        assert_eq!(code, crash::EXIT as i32);
        assert_eq!(reached.last().map(String::as_str), Some(label));
        p.lab.reopen();
        let r = replay(&p.lab.store, p.op).unwrap();
        assert_eq!(r.state, OpState::Publishing, "{label}");
        assert!(r.fenced, "{label}: the committed annotation was lost");
        let plan = p.lab.plan(p.op);
        let disk = PathDisk::new(&plan, &r);
        let recovery = p.lab.store.lock().unwrap().recover(p.op, &disk).unwrap();
        assert!(
            matches!(recovery.decision, Decision::Confirm { .. }),
            "{label}: {:?}",
            recovery.decision
        );
        drop(disk);
        let outcome = recover(&p.lab.store, p.op, &Control::new(false)).unwrap();
        assert_eq!(outcome.state, OpState::Committed, "{label}");
        assert_eq!(outcome.refusal, Some((Refusal::GrantRevoked, None)));
        p.oracle(label, &at_crash, &outcome);
        p.lab.reopen();
        let durable = status(&p.lab.store, p.op).unwrap();
        assert_eq!(durable.state, OpState::Committed);
        assert_eq!(durable.refusal, Some((Refusal::GrantRevoked, None)));
    }
}

/// The normal commit still validates its append, but needs no separate full replay for an absent
/// fence. Count real SQLite transactions on the adopted connection, without weakening its checks.
#[test]
fn a_commit_without_a_pending_fence_uses_one_journal_transaction() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    use std::sync::atomic::AtomicUsize;
    let Some(p) = Prepared::new(Case {
        scenario: Scenario::Confirm,
        removal: false,
        tree: None,
    }) else {
        return;
    };
    let begins = Arc::new(AtomicUsize::new(0));
    let count = begins.clone();
    let connection = rusqlite::Connection::open(p.lab.tmp.path().join("state/state.db")).unwrap();
    connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Transaction {
                    operation: TransactionOperation::Begin
                }
            ) {
                count.fetch_add(1, Ordering::SeqCst);
            }
            Authorization::Allow
        }))
        .unwrap();
    let store = Mutex::new(Store::new(connection).unwrap());
    let epoch = replay(&store, p.op).unwrap().epoch;
    begins.store(0, Ordering::SeqCst);
    Journal {
        store: &store,
        op: p.op,
        epoch,
    }
    .commit(&Control::new(false))
    .map_err(fault)
    .unwrap();
    assert_eq!(
        begins.load(Ordering::SeqCst),
        1,
        "an unfenced commit opened an extra replay transaction"
    );
    assert_eq!(replay(&store, p.op).unwrap().state, OpState::Committed);
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

/// Labels the default mode kills at first and last under both Q1 policies, not only at their class's
/// first label: the effect-before-journal gaps (an object on disk whose identity is not yet journaled)
/// and the folder metadata sets that change journaled identities' creation times one by one. Also the
/// window before `committed` (#526, docs/47 section 5): the confirmation passed, the record not yet
/// appended. It shares CP-13 with `renamed`, whose earlier kill would otherwise be the class's only one.
const GAPS: [&str; 5] = [
    "staging folder made",
    "created item",
    "directory metadata set",
    "staging metadata set",
    "before committed",
];

/// Default: each scenario killed once per crash-point class it reaches, plus the first and last of each
/// `GAPS` label (Run under both Q1 policies). Full: every boundary of every scenario under both.
#[test]
fn crash_matrix_every_point_recovers_and_abandons_under_the_oracle() {
    if lab().is_none() {
        return;
    }
    let mut tally = Tally::default();
    let mut lines = Vec::new();
    let mut window = 0;
    for scenario in Scenario::ALL {
        let policies: &[bool] = match (full(), scenario) {
            (true, _) => &[false, true],
            (false, Scenario::Run | Scenario::AbandonVerified) => &[false, true],
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
            let gap = |n: usize| {
                let l = labels[n - 1].as_str();
                GAPS.contains(&l)
                    && (labels.iter().position(|x| x == l) == Some(n - 1)
                        || labels.iter().rposition(|x| x == l) == Some(n - 1))
            };
            // Run under removal on adds only the gap kills; its classes are already swept under off.
            let classes = !(scenario == Scenario::Run && removal);
            let picks: Vec<usize> = (1..=labels.len())
                .filter(|&n| full() || gap(n) || (classes && seen.insert(class(&labels[n - 1]))))
                .collect();
            for &n in &picks {
                window += usize::from(labels[n - 1] == "before committed");
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
    assert!(window > 0, "the window before committed was never killed");
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

// ---- journal append faults (#535) ----------------------------------------------------------------

/// What the fault hook saw: the record kind of every append attempt in the step, and the destination at
/// the moment of the failure.
#[derive(Default)]
struct Appends {
    labels: Vec<String>,
    at_fault: Option<Snap>,
    /// Append attempts after the failed one: the executor must stop at once.
    after: usize,
}

/// The journal record's `kind`, with the target state for a transition.
fn record_label(record: &Record) -> String {
    let v = serde_json::to_value(record).unwrap();
    let kind = v["kind"].as_str().unwrap();
    match v["to"].as_str() {
        Some(to) => format!("{kind}:{to}"),
        None => kind.to_string(),
    }
}

/// Count every append; fail the `n`th (1-based, counted from the step's start) at `stage` with `kind`.
fn append_hook(
    seen: Arc<Mutex<Appends>>,
    dest: PathBuf,
    fault: Option<(usize, Fail, Stage)>,
) -> Hook {
    Box::new(move |record, stage| {
        let mut a = seen.lock().unwrap();
        if stage == Stage::Row {
            a.after += usize::from(a.at_fault.is_some());
            a.labels.push(record_label(record));
        }
        let (n, kind, at) = fault?;
        if stage == at && a.labels.len() == n && a.at_fault.is_none() {
            a.at_fault = Some(dest_snapshot(&dest));
            return Some(kind);
        }
        None
    })
}

/// The scenario's step, in this process (what `crash_child` runs in a child).
fn act(p: &Prepared, control: &Control) -> Result<Outcome, Fault> {
    let store = &p.lab.store;
    match p.case.scenario.action() {
        "run" => run(store, p.op, control),
        "recover" => recover(store, p.op, control),
        _ => {
            let r = recover(store, p.op, control)?;
            if r.state.is_terminal() {
                return Ok(r);
            }
            cancel(store, p.op, &p.lab.plan(p.op), control)
        }
    }
}

/// The record kind of every journal append the case's step makes when nothing fails.
fn enumerate_appends(case: Case) -> Option<Vec<String>> {
    let p = Prepared::new(case)?;
    let seen = Arc::new(Mutex::new(Appends::default()));
    let hook = append_hook(seen.clone(), p.lab.dest(), None);
    p.lab.store.lock().unwrap().set_fault(Some(hook));
    act(&p, &Control::new(case.removal)).unwrap();
    let labels = std::mem::take(&mut seen.lock().unwrap().labels);
    assert!(!labels.is_empty(), "{case:?}: no journal append");
    Some(labels)
}

/// Fail the case's `n`th append as `kind` at `stage`, then hold the executor and the journal to the oracle:
/// the fault surfaces as that database error; no further append and no further effect follows it; the
/// failed append stored nothing (Ack: its record is durable); the journal replays and the receipt agrees;
/// then recovery and the owner's Abandon settle it under the same oracle as every kill.
fn fail_append(
    case: Case,
    n: usize,
    labels: &[String],
    kind: Fail,
    stage: Stage,
    tally: &mut Tally,
) {
    let Some(mut p) = Prepared::new(case) else {
        return;
    };
    let ctx = format!("{case:?} append {n} ({}) {kind:?}@{stage:?}", labels[n - 1]);
    let rows_before = p.lab.store.lock().unwrap().rows(p.op).unwrap().len();
    let seen = Arc::new(Mutex::new(Appends::default()));
    let hook = append_hook(seen.clone(), p.lab.dest(), Some((n, kind, stage)));
    p.lab.store.lock().unwrap().set_fault(Some(hook));
    let result = act(&p, &Control::new(case.removal));
    p.lab.store.lock().unwrap().set_fault(None);
    let want = match kind {
        Fail::Full => "DiskFull",
        Fail::Busy => "DatabaseBusy",
    };
    let a = seen.lock().unwrap();
    assert_eq!(a.labels[n - 1], labels[n - 1], "{ctx}: appends drifted");
    assert!(
        matches!(&result, Err(Fault::Journal(m)) if m.contains(want)),
        "{ctx}: the failed append was swallowed or misreported: {result:?}"
    );
    assert_eq!(
        a.after, 0,
        "{ctx}: the executor appended after a failed append"
    );
    let at_fault = a.at_fault.clone().expect("the fault never fired");
    drop(a);
    let now = dest_snapshot(&p.lab.dest());
    assert_eq!(
        now, at_fault,
        "{ctx}: an effect ran after its record failed"
    );
    let rows = p.lab.store.lock().unwrap().rows(p.op).unwrap().len();
    assert_eq!(
        rows,
        rows_before + (n - 1) + usize::from(stage == Stage::Ack),
        "{ctx}: the failed append left a different journal"
    );
    let receipt = status(&p.lab.store, p.op).unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
    p.oracle(&format!("{ctx} after fault"), &now, &receipt);
    let (recovered, abandoned) = p.recover_then_abandon(&ctx, &now);
    tally.record(&format!("before {}", labels[n - 1]), recovered, abandoned);
}

/// Every kind at every stage: the six ways an append can fail.
const VARIANTS: [(Fail, Stage); 6] = [
    (Fail::Full, Stage::Row),
    (Fail::Busy, Stage::Head),
    (Fail::Full, Stage::Head),
    (Fail::Busy, Stage::Ack),
    (Fail::Full, Stage::Ack),
    (Fail::Busy, Stage::Row),
];

/// The record classes the fault matrix must have failed: the "before <record>" boundaries of the kill map.
const APPEND_CLASSES: [&str; 13] = [
    "CP-01", "CP-02", "CP-03", "CP-04", "CP-10", "CP-11", "CP-13", "CP-15", "CP-17", "CP-19",
    "CP-20", "CP-21", "CP-22",
];

/// Default: each journal append of each scenario fails once, rotating through the six variants. Full:
/// every append under every variant, both Q1 policies.
#[test]
fn a_failed_journal_append_stops_the_executor_and_recovers_under_the_oracle() {
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
            let labels = enumerate_appends(case).unwrap();
            let before = tally.kills;
            for n in 1..=labels.len() {
                let picks = if full() {
                    &VARIANTS[..]
                } else {
                    &VARIANTS[n % VARIANTS.len()..][..1]
                };
                for &(kind, stage) in picks {
                    fail_append(case, n, &labels, kind, stage, &mut tally);
                }
            }
            lines.push(format!(
                "C6 append {scenario:?} removal={removal}: {} appends, {} faults",
                labels.len(),
                tally.kills - before
            ));
        }
    }
    report("append", &tally, &lines);
    for cp in APPEND_CLASSES {
        assert!(tally.by_class.contains_key(cp), "{cp} never faulted");
    }
}
