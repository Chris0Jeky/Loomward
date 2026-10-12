"""Reading-copy labels derive from the actual checkout, not historical counts (#429)."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

_spec = importlib.util.spec_from_file_location("build_book", Path(__file__).resolve().parents[1] / "scripts/build_book.py")
book = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(book)


class BuildBookInventory(unittest.TestCase):
    def fixture(self, root, count=2):
        for name, content in {
            "docs/00-first.md": "# First\nHistorical chapter.\n",
            "docs/23-later.md": "# Later\nLater evidence.\n",
            "docs/EXPANSION-SOURCES.md": "# Sources\n",
            "backlog/INDEX.md": "# Backlog\n",
            "handoff/LOCAL-AGENT-PROMPT.md": "# Historical prompt\n",
            "backlog/issues.json": json.dumps([{"id": f"LW-{n:03}"} for n in range(1, count + 1)]),
        }.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")

    def build(self, root, html=False):
        args = ["build_book.py", "--markdown", str(root / "output/book.md")]
        if html:
            args += ["--html", str(root / "output/book.html")]
        with patch.object(book, "ROOT", root), patch("sys.argv", args), contextlib.redirect_stdout(io.StringIO()):
            book.main()
        return (root / "output/book.md").read_text(encoding="utf-8")

    def test_markdown_count_tracks_manifest_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for count in (2, 3):
                self.fixture(root, count)
                text = self.build(root)
                self.assertIn(f"{count}-task implementation backlog", text)
                self.assertIn(f"{count}-item backlog index", text)
                self.assertIn("2 numbered chapters", text)
                self.assertNotIn("100-task", text)
                self.assertNotIn("**Current evidence:** 184", text)

    def test_missing_manifest_does_not_create_an_invented_reading_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / "backlog/issues.json").unlink()
            with self.assertRaises(FileNotFoundError):
                self.build(root)
            self.assertFalse((root / "output/book.md").exists())

    def test_invalid_or_duplicate_inventory_is_refused_before_output(self):
        for inventory in ({}, [None], [{"id": "LW-001"}, {"id": "LW-001"}], [{"title": "missing ID"}]):
            with self.subTest(inventory=inventory), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.fixture(root)
                (root / "backlog/issues.json").write_text(json.dumps(inventory), encoding="utf-8")
                with self.assertRaises(ValueError):
                    self.build(root)
                self.assertFalse((root / "output/book.md").exists())

    @unittest.skipUnless(importlib.util.find_spec("mistune"), "optional HTML renderer mistune is not installed")
    def test_html_cover_and_navigation_use_the_same_inventory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root, 3)
            self.build(root, html=True)
            text = (root / "output/book.html").read_text(encoding="utf-8")
            self.assertIn("<strong>3</strong><span>dependency-linked local tasks", text)
            self.assertIn("3-task implementation backlog", text)
            self.assertIn("<strong>2</strong><span>architecture &amp; research chapters", text)
            self.assertNotIn("<strong>100</strong>", text)
            self.assertNotIn("passing Python reference tests", text)


if __name__ == "__main__":
    unittest.main()
