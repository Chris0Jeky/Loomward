"""Offline decisions for the serial lane merge queue."""
import importlib.util
from pathlib import Path
import unittest


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


if __name__ == "__main__":
    unittest.main()
