"""Pin nightly queue ownership without issuing a publish or network request (#436)."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class NightlyConcurrency(unittest.TestCase):
    def setUp(self):
        self.text = (ROOT / ".github/workflows/nightly.yml").read_text(encoding="utf-8")

    def test_skipped_workflows_do_not_acquire_the_publish_queue(self):
        self.assertIsNone(re.search(r"^concurrency:", self.text, re.MULTILINE))

    def test_qualifying_job_serializes_without_interrupting_a_publish(self):
        job = self.text.split("  windows-nightly:", 1)[1]
        self.assertRegex(job, r"(?m)^    concurrency:\n      group: nightly\n      cancel-in-progress: false$")
        self.assertEqual(len(re.findall(r"^\s*concurrency:", self.text, re.MULTILINE)), 1)

    def test_job_admission_retains_success_push_and_repository_checks(self):
        match = re.search(r"(?m)^    if: >-\n((?:      .*\n)+)", self.text)
        self.assertIsNotNone(match)
        expected = """github.event_name == 'workflow_dispatch' ||
        (github.event.workflow_run.conclusion == 'success' &&
        github.event.workflow_run.event == 'push' &&
        github.event.workflow_run.head_repository.full_name == github.repository)"""
        self.assertEqual(" ".join(match.group(1).split()), " ".join(expected.split()))
        self.assertIn("    branches: [main]", self.text)


if __name__ == "__main__":
    unittest.main()
