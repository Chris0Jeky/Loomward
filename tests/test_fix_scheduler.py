import unittest

from loomward.scheduler import LeaseBroker, admit_workloads, vector


class VectorUnknownTests(unittest.TestCase):
    def test_omitted_capacity_dimension_stays_unknown(self):
        result = vector({"cpu_slots": 4}, unknown=True)
        self.assertEqual(result["cpu_slots"], 4)
        self.assertIsNone(result["memory_mib"])
        self.assertIsNone(result["gpu_mib"])
        self.assertIsNone(result["io_mib_s"])

    def test_demands_omitted_dimension_stays_zero(self):
        result = vector({"cpu_slots": 1})
        self.assertEqual(
            result,
            {"cpu_slots": 1, "memory_mib": 0, "gpu_mib": 0, "io_mib_s": 0},
        )

    def test_explicit_none_capacity_stays_unknown(self):
        result = vector({"cpu_slots": 4, "memory_mib": None}, unknown=True)
        self.assertIsNone(result["memory_mib"])
        self.assertIsNone(result["gpu_mib"])

    def test_partial_capacity_yields_unknown_not_insufficient(self):
        scenario = {
            "capacity": {"cpu_slots": 4},
            "jobs": [
                {
                    "id": "job",
                    "demand": {"cpu_slots": 1, "memory_mib": 100},
                    "priority": 0,
                    "waited_seconds": 0,
                }
            ],
        }
        outcome = admit_workloads(scenario)
        self.assertEqual(outcome["accepted"], [])
        self.assertEqual(len(outcome["deferred"]), 1)
        reasons = outcome["deferred"][0]["reasons"]
        self.assertIn("unknown_memory_mib", reasons)
        self.assertFalse([r for r in reasons if r.startswith("insufficient_")])

    def test_fully_specified_capacity_behaves_as_before(self):
        scenario = {
            "capacity": {
                "cpu_slots": 2,
                "memory_mib": 300,
                "gpu_mib": 0,
                "io_mib_s": 50,
            },
            "reserved": {},
            "telemetry_age_seconds": 0,
            "jobs": [
                {
                    "id": "job",
                    "demand": {"cpu_slots": 1, "memory_mib": 100},
                    "priority": 0,
                    "waited_seconds": 0,
                }
            ],
        }
        outcome = admit_workloads(scenario)
        self.assertEqual([j["job_id"] for j in outcome["accepted"]], ["job"])
        self.assertEqual(outcome["deferred"], [])
        self.assertEqual(
            outcome["capacity"],
            {
                "cpu_slots": 2,
                "memory_mib": 300,
                "gpu_mib": 0,
                "io_mib_s": 50,
            },
        )


class DoubleReleaseTests(unittest.TestCase):
    def test_second_release_reports_false_and_frees_nothing(self):
        broker = LeaseBroker(
            {"cpu_slots": 1, "memory_mib": 100, "gpu_mib": 0, "io_mib_s": 0}
        )
        lease = broker.acquire(
            "owner", "r1", {"cpu_slots": 1, "memory_mib": 100}, ttl_seconds=30
        )
        self.assertEqual(lease["status"], "granted")
        self.assertTrue(broker.release(lease["lease_id"], "owner", lease["token"]))
        self.assertFalse(broker.release(lease["lease_id"], "owner", lease["token"]))
        snapshot = broker.snapshot()
        self.assertEqual(
            snapshot["reserved"],
            {"cpu_slots": 0, "memory_mib": 0, "gpu_mib": 0, "io_mib_s": 0},
        )
        self.assertEqual(snapshot["active_leases"], 0)


if __name__ == "__main__":
    unittest.main()
