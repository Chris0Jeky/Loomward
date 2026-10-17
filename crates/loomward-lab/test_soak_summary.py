import copy
import unittest

from soak_summary import evaluate


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


if __name__ == "__main__":
    unittest.main()
