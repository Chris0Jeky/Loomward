"""The published work graph must schedule adapters before their consumers (#429)."""
import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REQUIRED_EDGES = {
    "LW-069": {"LW-067", "LW-113"},
    "LW-066": {"LW-069"},
    "LW-118": {"LW-069"},
    "LW-114": {"LW-069"},
}


class SearchDependencyPlan(unittest.TestCase):
    def setUp(self):
        self.items = json.loads((ROOT / "backlog/issues.json").read_text(encoding="utf-8"))
        self.by_id = {item["id"]: item for item in self.items}

    def test_required_index_adapter_consumer_edges_are_schedulable(self):
        for owner, required in REQUIRED_EDGES.items():
            with self.subTest(owner=owner):
                self.assertLessEqual(required, set(self.by_id[owner]["depends_on"]))

    def test_topological_order_places_index_and_grants_before_each_consumer(self):
        done, active, order = set(), set(), []
        def visit(ident):
            self.assertNotIn(ident, active, f"dependency cycle at {ident}")
            if ident in done:
                return
            active.add(ident)
            for parent in self.by_id[ident]["depends_on"]:
                visit(parent)
            active.remove(ident)
            done.add(ident)
            order.append(ident)
        for ident in self.by_id:
            visit(ident)
        for consumer in ("LW-066", "LW-118", "LW-114"):
            self.assertLess(order.index("LW-113"), order.index("LW-069"))
            self.assertLess(order.index("LW-067"), order.index("LW-069"))
            self.assertLess(order.index("LW-069"), order.index(consumer))

    def test_everything_remains_optional_for_lexical_search_and_consumers(self):
        def ancestors(ident):
            found, pending = set(), list(self.by_id[ident]["depends_on"])
            while pending:
                parent = pending.pop()
                if parent not in found:
                    found.add(parent)
                    pending.extend(self.by_id[parent]["depends_on"])
            return found
        for ident in ("LW-113", "LW-069", "LW-066", "LW-118"):
            self.assertNotIn("LW-114", ancestors(ident), ident)

    def test_touched_dependency_mirrors_match_the_manifest(self):
        index = (ROOT / "backlog/INDEX.md").read_text(encoding="utf-8")
        for ident in REQUIRED_EDGES:
            item = self.by_id[ident]
            body = (ROOT / item["body_file"]).read_text(encoding="utf-8")
            line = next(line for line in body.splitlines()
                        if line.startswith(("Dependencies:", "**Depends on:**")))
            row = next(line for line in index.splitlines() if line.startswith(f"| [{ident}]"))
            expected = set(item["depends_on"])
            self.assertEqual(set(re.findall(r"LW-\d{3}", line)), expected, ident)
            self.assertEqual(set(re.findall(r"LW-\d{3}", row.split("|")[-2])), expected, ident)


if __name__ == "__main__":
    unittest.main()
