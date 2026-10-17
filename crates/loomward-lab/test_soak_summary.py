import copy
import unittest

from soak_summary import LEGACY_NOTE, evaluate

BASE_MS = 1_790_000_000_000  # fixed epoch-millisecond origin for synthetic wall clocks
WALL_GAP = "wall-clock continuity gap or divergence from monotonic time"
MONO_GAP = "sample continuity gap or invalid timestamp"


def with_wall_clock(records, shift_from=None, shift_s=0):
    """Give the start, samples and end a wall clock that tracks elapsed_s.

    From shift_from onwards the wall clock is stepped by shift_s while elapsed_s is left alone,
    which is what a suspend looks like when the monotonic counter does not count it.
    """
    result = []
    for record in records:
        record = dict(record)
        if record["kind"] == "start":
            record["unix_seconds"] = BASE_MS // 1000
        if record["kind"] in ("sample", "end"):
            wall = BASE_MS + round(record["elapsed_s"] * 1000)
            if shift_from is not None and record["elapsed_s"] >= shift_from:
                wall += round(shift_s * 1000)
            record["unix_ms"] = wall
        result.append(record)
    return result


def fixture():
    records = [{"kind": "start", "schema_version": 1, "requested_seconds": 900,
                "manifest": {"expected": {"files": 128, "directories": 4, "logical_bytes": 500}}}]
    stages = ["grant", "scan", "tree.slice", "search.query", "watch", "refresh", "revoke", "forget", "restart"]
    for cycle in range(1, 181):
        for stage in stages:
            empty = stage in ("grant", "forget", "restart")
            records.append({"kind": "sample", "cycle": cycle, "operation": stage,
                            "elapsed_s": (cycle - 1) * 5 + stages.index(stage) * .01,
                            "latency_ms": 1, "error": None, "handles": 100, "logical_bytes": 0 if empty else 500,
                            "rss_bytes": 10_000_000, "private_bytes": 10_000_000,
                            "sizes": {"catalog.db": 100, "state.db": 100,
                                      "catalog.db-wal": 0, "state.db-wal": 0},
                            "rows": {"root": 0 if empty else 1, "dir": 0 if empty else 5,
                                     "file": 0 if empty else 128, "scan_run": 0 if empty else 1,
                                     "stage_entry": 0, "identifiable_grant": 0 if empty else 1}})
    records.append({"kind": "end", "cycles": 180, "elapsed_s": 900, "success": True, "error": None})
    return records


class SummaryTests(unittest.TestCase):
    def test_full_duration_rule_with_streamed_synthetic_receipt(self):
        def records():
            template = fixture()
            template[0]["requested_seconds"] = 72 * 3600
            yield template[0]
            for cycle in range(4320):
                for index, sample in enumerate(template[1:10]):
                    yield dict(sample, cycle=cycle + 1, elapsed_s=cycle * 60 + index * .01)
            yield dict(template[-1], cycles=4320, elapsed_s=72 * 3600)
        self.assertEqual(evaluate(records())["status"], "PASS")

    def test_smoke_pass_cannot_close_72_hour_row(self):
        self.assertEqual(evaluate(fixture(), smoke=True)["status"], "SMOKE PASS")
        self.assertEqual(evaluate(fixture())["status"], "FAIL")

    def test_rejects_incomplete_errors_growth_sizes_rows_and_missing_operations(self):
        original = fixture()
        cases = []
        cases.append(original[:-1])
        for field, value in [("error", "injected"), ("handles", 1000),
                             ("private_bytes", 100_000_000), ("rss_bytes", 200_000_000)]:
            records = copy.deepcopy(original)
            for sample in records[1:-1]:
                if sample["elapsed_s"] >= 720:
                    sample[field] = value
            cases.append(records)
        for group, field, value in [("sizes", "catalog.db-wal", 70_000_000),
                                    ("sizes", "state.db", 70_000_000), ("rows", "file", 129)]:
            records = copy.deepcopy(original)
            records[2][group][field] = value
            cases.append(records)
        records = copy.deepcopy(original)
        records[8]["rows"]["identifiable_grant"] = 1
        cases.append(records)
        records = copy.deepcopy(original)
        del records[5]
        cases.append(records)
        for field, value in [("logical_bytes", 501), ("handles", 0)]:
            records = copy.deepcopy(original)
            records[2][field] = value
            cases.append(records)
        records = copy.deepcopy(original)
        records[0]["requested_seconds"] = float("nan")
        cases.append(records)
        for records in cases:
            with self.subTest(records=len(records)):
                self.assertEqual(evaluate(records, smoke=True)["status"], "FAIL")


class WallClockContinuityTests(unittest.TestCase):
    def test_clean_wall_clock_receipt_passes_without_a_legacy_note(self):
        result = evaluate(with_wall_clock(fixture()), smoke=True)
        self.assertEqual(result["status"], "SMOKE PASS")
        self.assertEqual(result["notes"], [])

    def test_planted_sleep_gap_fails_when_only_the_wall_clock_jumps(self):
        # A suspend: the wall clock advances an hour, the monotonic elapsed_s does not.
        records = with_wall_clock(fixture(), shift_from=245, shift_s=3600)
        result = evaluate(records, smoke=True)
        self.assertEqual(result["status"], "FAIL")
        self.assertIn(WALL_GAP, result["errors"])
        self.assertNotIn(MONO_GAP, result["errors"])

    def test_small_backward_wall_step_is_an_ntp_correction_not_a_failure(self):
        records = with_wall_clock(fixture(), shift_from=245, shift_s=-1)
        self.assertNotIn(WALL_GAP, evaluate(records, smoke=True)["errors"])

    def test_large_backward_wall_step_fails_as_divergence(self):
        records = with_wall_clock(fixture(), shift_from=245, shift_s=-3600)
        result = evaluate(records, smoke=True)
        self.assertEqual(result["status"], "FAIL")
        self.assertIn(WALL_GAP, result["errors"])

    def test_end_record_wall_clock_gap_fails(self):
        records = with_wall_clock(fixture())
        records[-1]["unix_ms"] += 3600 * 1000
        self.assertIn(WALL_GAP, evaluate(records, smoke=True)["errors"])

    def test_end_record_without_unix_ms_on_a_wall_clock_receipt_fails(self):
        records = with_wall_clock(fixture())
        del records[-1]["unix_ms"]
        self.assertIn("end record lacks unix_ms", evaluate(records, smoke=True)["errors"])

    def test_sample_missing_unix_ms_after_wall_clock_samples_fails(self):
        records = with_wall_clock(fixture())
        del records[5]["unix_ms"]
        self.assertIn("wall-clock field missing or mixed across samples",
                      evaluate(records, smoke=True)["errors"])

    def test_unix_ms_on_some_samples_of_a_legacy_run_fails(self):
        records = fixture()
        records[3]["unix_ms"] = BASE_MS
        self.assertIn("wall-clock field missing or mixed across samples",
                      evaluate(records, smoke=True)["errors"])

    def test_non_integer_wall_clock_fails(self):
        records = with_wall_clock(fixture())
        records[4]["unix_ms"] = "soon"
        self.assertIn("invalid wall-clock timestamp", evaluate(records, smoke=True)["errors"])


class LegacyReceiptTests(unittest.TestCase):
    def test_legacy_receipt_is_evaluated_on_monotonic_time_with_a_note(self):
        result = evaluate(fixture(), smoke=True)
        self.assertEqual(result["status"], "SMOKE PASS")
        self.assertEqual(result["notes"], ["wall-clock continuity not checked (legacy receipt)"])
        self.assertEqual(LEGACY_NOTE, "wall-clock continuity not checked (legacy receipt)")

    def test_legacy_receipt_still_fails_on_a_planted_monotonic_gap(self):
        records = fixture()
        for record in records[1:]:
            if record["kind"] in ("sample", "end") and record["elapsed_s"] >= 245:
                record["elapsed_s"] += 3600
        result = evaluate(records, smoke=True)
        self.assertEqual(result["status"], "FAIL")
        self.assertIn(MONO_GAP, result["errors"])
        self.assertEqual(result["notes"], [LEGACY_NOTE])


if __name__ == "__main__":
    unittest.main()
