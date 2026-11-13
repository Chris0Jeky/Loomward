"""Offline decisions for the serial lane merge queue."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/lanes/merge_queue.py"
SPEC = importlib.util.spec_from_file_location("lane_merge_queue", SCRIPT)
queue = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(queue)


def ready(**changes):
    facts = {
        "state": "OPEN",
        "head": "a" * 40,
        "reviewed": "a" * 40,
        "reviewed_ancestor": True,
        "commits": [],
        "merge_state": "CLEAN",
        "behind": 0,
        "checks": [{"status": "COMPLETED", "conclusion": "SUCCESS"}],
        "observed_seconds": 180,
    }
    facts.update(changes)
    return facts


def main_merge(**changes):
    commit = {"parent_count": 2, "second_parent_on_main": True, "tree_matches": True}
    commit.update(changes)
    return commit


class MergeDecisionTests(unittest.TestCase):
    def test_reviewed_green_aged_up_to_date_merges(self):
        self.assertEqual(queue.decide(ready())[0], "merge")

    def test_extra_non_merge_commit_parks(self):
        self.assertEqual(queue.decide(ready(
            head="b" * 40, commits=[main_merge(parent_count=1)]
        )), ("park", "head has unreviewed commits"))

    def test_only_clean_merges_from_main_merge(self):
        self.assertEqual(queue.decide(ready(
            head="b" * 40, commits=[main_merge(), main_merge()]
        ))[0], "merge")

    def test_dirty_merge_tree_mismatch_parks(self):
        self.assertEqual(queue.decide(ready(
            head="b" * 40, commits=[main_merge(tree_matches=False)]
        )), ("park", "head has unreviewed commits"))

    def test_merge_from_other_branch_parks(self):
        self.assertEqual(queue.decide(ready(
            head="b" * 40, commits=[main_merge(second_parent_on_main=False)]
        ))[0], "park")

    def test_reviewed_not_ancestor_parks(self):
        self.assertEqual(queue.decide(ready(reviewed_ancestor=False))[0], "park")

    def test_changed_head_without_commit_proof_parks(self):
        self.assertEqual(queue.decide(ready(head="b" * 40))[0], "park")

    def test_behind_updates(self):
        self.assertEqual(queue.decide(ready(behind=2))[0], "update")

    def test_unreviewed_head_is_not_updated(self):
        self.assertEqual(queue.decide(ready(
            head="b" * 40, behind=2, commits=[main_merge(parent_count=1)]
        ))[0], "park")

    def test_conflicts_park_even_when_behind(self):
        self.assertEqual(queue.decide(ready(merge_state="DIRTY", behind=1))[0], "park")

    def test_pending_waits(self):
        self.assertEqual(queue.decide(ready(checks=[
            {"status": "IN_PROGRESS", "conclusion": None}
        ]))[0], "wait")

    def test_failed_parks(self):
        self.assertEqual(queue.decide(ready(checks=[
            {"status": "COMPLETED", "conclusion": "FAILURE"}
        ]))[0], "park")

    def test_cancelled_reruns(self):
        self.assertEqual(queue.decide(ready(checks=[
            {"status": "COMPLETED", "conclusion": "CANCELLED"}
        ]))[0], "rerun")

    def test_failure_wins_over_pending_and_cancelled(self):
        self.assertEqual(queue.decide(ready(checks=[
            {"status": "IN_PROGRESS"},
            {"status": "COMPLETED", "conclusion": "CANCELLED"},
            {"status": "COMPLETED", "conclusion": "FAILURE"},
        ]))[0], "park")

    def test_head_observed_less_than_180_seconds_waits(self):
        self.assertEqual(queue.decide(ready(observed_seconds=179.99))[0], "wait")

    def test_merged_or_closed_is_done_without_other_facts(self):
        for state in ("MERGED", "CLOSED"):
            with self.subTest(state=state):
                self.assertEqual(queue.decide({"state": state})[0], "done")

    def test_empty_checks_wait(self):
        self.assertEqual(queue.decide(ready(checks=[]))[0], "wait")

    def test_legacy_status_contexts(self):
        for state, action in (("SUCCESS", "merge"), ("PENDING", "wait"), ("ERROR", "park")):
            with self.subTest(state=state):
                self.assertEqual(queue.decide(ready(checks=[{"state": state}]))[0], action)

    def test_skipped_and_neutral_are_green(self):
        self.assertEqual(queue.decide(ready(checks=[
            {"status": "COMPLETED", "conclusion": "SUCCESS"},
            {"status": "COMPLETED", "conclusion": "SKIPPED"},
            {"status": "COMPLETED", "conclusion": "NEUTRAL"},
        ]))[0], "merge")

    def test_only_skipped_checks_park_as_absent_ci(self):
        self.assertEqual(queue.decide(ready(checks=[
            {"status": "COMPLETED", "conclusion": "SKIPPED"},
            {"status": "COMPLETED", "conclusion": "NEUTRAL"},
        ]))[0], "park")

    def test_unknown_facts_wait(self):
        for changes in ({"state": "UNKNOWN"}, {"behind": None}, {"merge_state": "UNKNOWN"},
                        {"checks": [{}]}):
            with self.subTest(changes=changes):
                self.assertEqual(queue.decide(ready(**changes))[0], "wait")

    def test_first_observation_starts_at_zero(self):
        observation, age = queue.observe({}, "123", "a" * 40, 1000)
        self.assertEqual(observation, {"head": "a" * 40, "first_seen": 1000})
        self.assertEqual(age, 0)

    def test_same_head_keeps_observation_time(self):
        observation, age = queue.observe(
            {"123": {"head": "a" * 40, "first_seen": 1000}}, "123", "a" * 40, 1180)
        self.assertEqual(observation["first_seen"], 1000)
        self.assertEqual(age, 180)

    def test_head_change_resets_observation_time(self):
        seen = {"123": {"head": "a" * 40, "first_seen": 1000}}
        observation, age = queue.observe(seen, "123", "b" * 40, 5000)
        self.assertEqual(observation, {"head": "b" * 40, "first_seen": 5000})
        self.assertEqual(age, 0)
        self.assertEqual(seen["123"]["head"], "a" * 40)


class MergeTeardownTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.cwd = Path(temporary.name)
        self.home = self.cwd / "lane-home"
        self.home.mkdir()
        self.repo = self.cwd / "coordinator"
        self.repo.mkdir()
        self.file = self.home / "mergeq.txt"
        self.sha = "a" * 40
        self.file.write_text(f"123 {self.sha} Example_1-test\n", encoding="utf-8")
        (self.home / "mergeq.seen.json").write_text(json.dumps({
            "123": {"head": self.sha, "first_seen": 1000},
            "124": {"head": self.sha, "first_seen": 1000},
        }), encoding="utf-8")
        self.teardown_code = 0
        self.merge_code = 0
        self.state = "OPEN"
        self.bash = self.enterContext(mock.patch.object(queue.shutil, "which", return_value="bash"))
        self.runner = self.enterContext(mock.patch.object(
            queue.subprocess, "run", side_effect=self.run_command))
        self.logs = self.enterContext(mock.patch.object(queue, "log"))
        self.enterContext(mock.patch.object(queue.time, "time", return_value=1180))
        self.enterContext(mock.patch.object(queue.Path, "cwd", return_value=self.cwd))
        self.enterContext(mock.patch.object(queue, "gather", side_effect=lambda *args: ready(
            state=self.state)))

    def run_command(self, args, **kwargs):
        if args[:3] == ("gh", "pr", "view"):
            output = json.dumps({"state": self.state, "headRefOid": self.sha})
            return subprocess.CompletedProcess(args, 0, output, "")
        if args[:2] == ("gh", "api"):
            return subprocess.CompletedProcess(args, 0, '{"behind_by": 0}', "")
        if args[0] == "py":
            return subprocess.CompletedProcess(args, self.merge_code, "merge helper result", "")
        if args[0] == self.bash.return_value:
            return subprocess.CompletedProcess(args, self.teardown_code, "ignored files\n",
                                               "work remains in worktree\n" if self.teardown_code
                                               else "removed Example_1-test\n")
        self.fail(f"unexpected command: {args}")

    def one_pass(self, dry_run=False):
        queue.one_pass(str(self.repo), self.home, "example/project", dry_run)

    def teardowns(self):
        return [call for call in self.runner.call_args_list
                if call.args[0][0] == self.bash.return_value]

    def test_lane_merges_then_tears_down_with_queue_cwd_and_home(self):
        self.bash.return_value = "C:/synthetic Git/bin/bash.exe"
        self.one_pass()
        self.assertEqual(self.file.read_text(encoding="utf-8"), "")
        calls = self.teardowns()
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0].args[0], (
            self.bash.return_value, str(self.repo / "scripts/lanes/rmwt.sh"), "Example_1-test"))
        self.bash.assert_called_once_with("bash")
        self.assertEqual(calls[0].kwargs["cwd"], self.cwd)
        self.assertEqual(calls[0].kwargs["env"]["LW_HOME"], str(self.home))
        commands = [call.args[0][0] for call in self.runner.call_args_list]
        self.assertLess(commands.index("py"), commands.index(self.bash.return_value))
        self.assertIn(mock.call("123", "teardown",
                                "Example_1-test exited 0: removed Example_1-test", False),
                      self.logs.call_args_list)

    def test_two_fields_merge_without_teardown(self):
        self.file.write_text(f"123 {self.sha}\n", encoding="utf-8")
        self.one_pass()
        self.assertEqual(self.file.read_text(encoding="utf-8"), "")
        self.assertEqual(self.teardowns(), [])
        self.assertTrue(any(call.args[0][0] == "py" for call in self.runner.call_args_list))

    def test_teardown_failure_is_logged_once_and_next_line_merges(self):
        self.teardown_code = 3
        self.file.write_text(f"123 {self.sha} Example_1-test\n124 {self.sha}\n", encoding="utf-8")
        self.one_pass()
        self.assertEqual(self.file.read_text(encoding="utf-8"), f"124 {self.sha}\n")
        self.assertIn(mock.call("123", "teardown",
                                "Example_1-test exited 3: work remains in worktree", False),
                      self.logs.call_args_list)
        self.one_pass()
        self.assertEqual(self.file.read_text(encoding="utf-8"), "")
        self.assertEqual(len(self.teardowns()), 1)
        self.assertEqual(sum(call.args[0][0] == "py" for call in self.runner.call_args_list), 2)

    def test_dry_run_logs_teardown_without_calling_it(self):
        before = self.file.read_text(encoding="utf-8")
        self.one_pass(dry_run=True)
        self.assertEqual(self.file.read_text(encoding="utf-8"), before)
        self.assertEqual(self.teardowns(), [])
        merges = [call for call in self.runner.call_args_list if call.args[0][0] == "py"]
        self.assertEqual(len(merges), 1)
        self.assertIn("--dry-run", merges[0].args[0])
        reasons = [call.args[2] for call in self.logs.call_args_list]
        self.assertTrue(any("would run" in reason and "rmwt.sh" in reason
                            and "Example_1-test" in reason for reason in reasons))

    def test_invalid_lanes_park_before_any_subprocess(self):
        for lane in ("../other", "bad/name", "bad.name", "bad;name", "caf\u00e9"):
            with self.subTest(lane=lane):
                self.file.write_text(f"123 {self.sha} {lane}\n", encoding="utf-8")
                self.one_pass()
                self.assertEqual(self.file.read_text(encoding="utf-8"), "")
                self.assertIn("invalid lane", (self.home / "mergeq.parked").read_text(
                    encoding="utf-8").splitlines()[-1])
                self.runner.assert_not_called()

    def test_failed_merge_never_tears_down_or_pops(self):
        self.merge_code = 1
        before = self.file.read_text(encoding="utf-8")
        self.one_pass()
        self.assertEqual(self.file.read_text(encoding="utf-8"), before)
        self.assertEqual(self.teardowns(), [])
        self.assertEqual(self.logs.call_args.args[1], "wait")

    def test_already_merged_or_closed_never_tears_down(self):
        for state in ("MERGED", "CLOSED"):
            with self.subTest(state=state):
                self.state = state
                self.file.write_text(f"123 {self.sha} Example_1-test\n", encoding="utf-8")
                self.one_pass()
                self.assertEqual(self.file.read_text(encoding="utf-8"), "")
                self.assertEqual(self.teardowns(), [])


if __name__ == "__main__":
    unittest.main()
