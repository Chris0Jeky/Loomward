"""Pure tests for `scripts/verify.py --changed` scope selection (#1016): no git, no cargo run."""
import contextlib
import importlib.util
import io
import unittest
from pathlib import Path
from subprocess import CompletedProcess

ROOT = Path(__file__).resolve().parents[1]
_spec = importlib.util.spec_from_file_location("verify", ROOT / "scripts" / "verify.py")
verify = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(verify)

WS = "/ws"


def pkg(name, *deps, dev=()):
    """A `cargo metadata --no-deps` package: path deps to workspace crates and one registry dep."""
    entries = [{"name": d, "path": f"{WS}/crates/{d}", "kind": None} for d in deps]
    entries += [{"name": d, "path": f"{WS}/crates/{d}", "kind": "dev"} for d in dev]
    entries.append({"name": "serde", "path": None, "kind": None})
    return {"name": name, "manifest_path": f"{WS}/crates/{name}/Cargo.toml", "dependencies": entries}


# protocol <- core <- engine <- service; windows <- engine (dev); learn and core-extra stand alone.
PACKAGES = [
    pkg("proto"), pkg("core", "proto"), pkg("engine", "core", dev=("windows",)), pkg("windows"),
    pkg("service", "engine"), pkg("learn"), pkg("core-extra"),
]


def select(changed, reads=None):
    return verify.select_scope(changed, PACKAGES, WS, reads or {})


class MappingTests(unittest.TestCase):
    def test_file_maps_to_its_crate_and_dependents_only(self):
        scope = select(["crates/engine/src/lib.rs"])
        self.assertEqual(scope.crates, ("engine", "service"))
        self.assertFalse(scope.full)
        self.assertFalse(scope.python)

    def test_a_leaf_dependency_pulls_in_the_whole_chain(self):
        self.assertEqual(select(["crates/proto/src/a.rs"]).crates, ("core", "engine", "proto", "service"))

    def test_dev_dependency_edges_count_as_dependents(self):
        self.assertEqual(select(["crates/windows/src/a.rs"]).crates, ("engine", "service", "windows"))

    def test_longest_directory_wins_so_core_extra_is_not_core(self):
        self.assertEqual(select(["crates/core-extra/src/a.rs"]).crates, ("core-extra",))

    def test_registry_dependency_names_never_become_members(self):
        self.assertNotIn("serde", select(["crates/learn/src/a.rs"]).crates)

    def test_windows_separators_are_normalised(self):
        self.assertEqual(select(["crates\\learn\\src\\a.rs"]).crates, ("learn",))

    def test_nothing_changed_selects_nothing(self):
        scope = select([])
        self.assertEqual((scope.full, scope.crates, scope.python), (False, (), False))


class FallbackTests(unittest.TestCase):
    TRIGGERS = [
        "Cargo.toml", "Cargo.lock", "contracts/v3/examples/a.json", "fixtures/x.json", ".cargo/config.toml",
        "rust-toolchain.toml", "rust-toolchain", "schemas/a.json", "rustfmt.toml", "clippy.toml",
        "crates/stray/file.rs", "unknown-top/dir/file.txt",
    ]

    def test_every_fallback_trigger_selects_the_full_workspace(self):
        for path in self.TRIGGERS:
            with self.subTest(path=path):
                scope = select([path, "crates/learn/src/a.rs"])
                self.assertTrue(scope.full)
                self.assertEqual(set(scope.crates), {p["name"] for p in PACKAGES})
                self.assertIn(path, " ".join(scope.reasons))

    def test_crate_manifest_is_not_a_fallback_but_a_crate_change(self):
        scope = select(["crates/learn/Cargo.toml"])
        self.assertEqual((scope.full, scope.crates), (False, ("learn",)))

    def test_inert_paths_nobody_reads_run_no_crate(self):
        for path in ["docs/01-x.md", "README.md", "app/src/a.ts", "backlog/issues.json", ".github/workflows/ci.yml", "LICENSE"]:
            with self.subTest(path=path):
                scope = select([path])
                self.assertEqual((scope.full, scope.crates), (False, ()))


class ReaderTests(unittest.TestCase):
    def test_a_doc_named_by_a_crate_selects_that_crate_and_its_dependents(self):
        reads = {"engine": ({"docs/47-copy-and-verify.md"}, False, False)}
        scope = select(["docs/47-copy-and-verify.md"], reads)
        self.assertEqual((scope.full, scope.crates), (False, ("engine",)))  # a reader, not its dependents

    def test_cross_crate_include_str_selects_the_reader(self):
        reads = {"engine": ({scan_ref("../../windows/src/recycle_bin.rs")}, False, False)}
        scope = select(["crates/windows/src/recycle_bin.rs"], reads)
        self.assertEqual(scope.crates, ("engine", "service", "windows"))

    def test_a_directory_literal_covers_files_under_it(self):
        reads = {"learn": ({"docs/examples/"}, False, False)}
        self.assertEqual(select(["docs/examples/a.json"], reads).crates, ("learn",))

    def test_python_source_change_selects_crates_that_shell_out_to_python(self):
        reads = {"engine": (set(), False, True)}
        scope = select(["python/loomward/planner_v2.py"], reads)
        self.assertEqual(scope.crates, ("engine",))
        self.assertTrue(scope.python)

    def test_tree_scanning_crate_joins_any_rust_change_but_not_a_docs_change(self):
        reads = {"windows": (set(), True, False)}
        self.assertEqual(select(["crates/learn/src/a.rs"], reads).crates, ("learn", "windows"))
        self.assertEqual(select(["native/src/main.rs"], reads).crates, ("windows",))
        self.assertEqual(select(["docs/a.md"], reads).crates, ())


def scan_ref(text):
    return verify.norm_ref(text)


class PythonTriggerTests(unittest.TestCase):
    def test_python_runs_for_each_python_input(self):
        for path in ["python/loomward/a.py", "tests/test_a.py", "fixtures/a.json", "contracts/v3/a.json", "scripts/a.py", "pyproject.toml"]:
            with self.subTest(path=path):
                self.assertTrue(select([path]).python)

    def test_a_crate_only_change_does_not_run_python(self):
        self.assertFalse(select(["crates/learn/src/a.rs", "Cargo.lock"]).python)


class ScanSourcesTests(unittest.TestCase):
    def test_literals_are_normalised_and_prose_is_dropped(self):
        lits, _, _ = verify.scan_sources(['include_str!("../../../schemas/a-v1.json"); let m = "not a path here.";\nlet x = "/../../contracts/v3/examples";'])
        self.assertIn("schemas/a-v1.json", lits)
        self.assertIn("contracts/v3/examples", lits)
        self.assertNotIn("not a path here.", lits)

    def test_tree_scan_and_python_shell_out_are_detected(self):
        scan = 'let w = Path::new(env!("CARGO_MANIFEST_DIR")).parent()\n.unwrap()\n.parent()\n.unwrap();\nfor e in std::fs::read_dir(w) {}'
        self.assertTrue(verify.scan_sources([scan])[1])
        self.assertTrue(verify.scan_sources(['std::fs::read_dir(p); let r = base.join("../..");'])[1])
        self.assertFalse(verify.scan_sources(['let r = base.join("../../contracts/v3");'])[1])
        self.assertFalse(verify.scan_sources(['let x = root.join("src"); std::fs::read_dir(x);'])[1])
        self.assertTrue(verify.scan_sources(['Command::new("py").args(["-3"])'])[2])
        self.assertFalse(verify.scan_sources(['Command::new("cargo")'])[2])


class SummaryTests(unittest.TestCase):
    def test_selected_run_says_selection_only_and_never_a_workspace_pass(self):
        text = "\n".join(verify.scope_summary(select(["crates/learn/src/a.rs"]), "origin/main"))
        self.assertIn("selected crates only (learn); hosted CI runs the whole workspace", text)
        self.assertIn("Python suite: not run", text)
        self.assertNotIn("full workspace", text)
        self.assertIn("crates/learn/src/a.rs -> crate learn", text)

    def test_fallback_says_full_workspace_and_why(self):
        text = "\n".join(verify.scope_summary(select(["Cargo.lock"]), "origin/main"))
        self.assertIn("SCOPE Rust: full workspace (fallback above)", text)
        self.assertIn("Cargo.lock: shared build or data input", text)
        self.assertNotIn("selected crates only", text)

    def test_native_change_is_reported_as_not_run(self):
        text = "\n".join(verify.scope_summary(select(["native/src/main.rs"]), "origin/main"))
        self.assertIn("native was not run", text)

    def test_unknown_scope_reports_the_full_run(self):
        text = "\n".join(verify.scope_summary(None, "origin/main", "git failed"))
        self.assertIn("full workspace and Python suite", text)
        self.assertIn("git failed", text)


class MainWiringTests(unittest.TestCase):
    def run_main(self, scope, argv=("--changed",), why=None):
        calls = []

        def run(command, **kwargs):
            calls.append(list(command))
            return CompletedProcess(command, 0)

        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = verify.main(list(argv), which=lambda n: "/fake/cargo" if n == "cargo" else None, run=run,
                               scope_for=lambda base: (scope, why))
        return code, calls, out.getvalue()

    def test_selected_run_uses_p_flags_skips_python_and_restates_the_scope_last(self):
        code, calls, out = self.run_main(select(["crates/engine/src/a.rs"]))
        self.assertEqual(code, 0)
        flat = [" ".join(c) for c in calls]
        self.assertIn("cargo test -p engine -p service --no-fail-fast", flat)
        self.assertIn("cargo clippy -p engine -p service --all-targets -- -D warnings", flat)
        self.assertIn("cargo fmt --all -- --check", flat)
        self.assertFalse(any("--workspace" in c or "unittest" in c for c in flat))
        self.assertIn("selected crates only", out.strip().splitlines()[-1])

    def test_fallback_runs_the_default_rust_commands(self):
        _, calls, _ = self.run_main(select(["Cargo.lock"]))
        self.assertIn(["cargo", "test", "--workspace", "--no-fail-fast"], calls)
        self.assertFalse(any("unittest" in " ".join(c) for c in calls))

    def test_python_input_runs_the_python_suite(self):
        _, calls, _ = self.run_main(select(["scripts/a.py"]))
        self.assertTrue(any("unittest" in " ".join(c) for c in calls))
        self.assertFalse(any(c[:2] == ["cargo", "test"] for c in calls))

    def test_unknown_scope_falls_back_to_the_whole_default_run(self):
        _, calls, out = self.run_main(None, why="boom")
        self.assertTrue(any("unittest" in " ".join(c) for c in calls))
        self.assertIn(["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"], calls)
        self.assertIn("boom", out)

    def test_without_the_flag_nothing_is_selected(self):
        def never(base):
            raise AssertionError("scope computed without --changed")

        calls = []
        with contextlib.redirect_stdout(io.StringIO()):
            verify.main([], which=lambda n: "/fake/cargo" if n == "cargo" else None,
                        run=lambda c, **k: calls.append(list(c)) or CompletedProcess(c, 0), scope_for=never)
        self.assertEqual(len(calls), 4)


if __name__ == "__main__":
    unittest.main()
