"""The installed Ask provider needs its broker beside the desktop executable."""
import importlib.util
import json
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("build_broker", ROOT / "scripts/build_broker.py")
broker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(broker)


class DesktopPackaging(unittest.TestCase):
    def test_ask_broker_is_an_external_binary(self):
        config = json.loads((ROOT / "native/tauri.conf.json").read_text(encoding="utf-8"))
        self.assertIn("binaries/loomward-broker", config["bundle"].get("externalBin", []))

    def test_mcp_is_an_external_binary(self):
        config = json.loads((ROOT / "native/tauri.conf.json").read_text(encoding="utf-8"))
        self.assertIn("binaries/loomward-mcp", config["bundle"].get("externalBin", []))

    def test_native_lock_includes_the_ask_dependency(self):
        packages = tomllib.loads((ROOT / "native/Cargo.lock").read_text(encoding="utf-8"))["package"]
        self.assertIn("loomward-ask", {p["name"] for p in packages})
        service = next(p for p in packages if p["name"] == "loomward-service")
        self.assertIn("loomward-ask", service["dependencies"])

    def test_ci_and_installer_stage_before_building_the_desktop(self):
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8").split("  desktop:")[1].split("  app:")[0]
        self.assertLess(ci.index("python scripts/build_broker.py"), ci.index("cargo +stable build --locked"))
        action = (ROOT / ".github/actions/desktop-installer/action.yml").read_text(encoding="utf-8")
        self.assertLess(action.index("python scripts/build_broker.py --release"), action.index("tauri.js build"))
        self.assertIn("Join-Path $dir 'loomward-broker.exe'", action)
        self.assertIn("Join-Path $dir 'loomward-desktop.exe'", action)


class StageBroker(unittest.TestCase):
    def test_stages_the_selected_profile_from_cargos_actual_target_directory(self):
        for release in (False, True):
            with self.subTest(release=release), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                target = root / "custom target"
                source = target / ("release" if release else "debug") / "loomward-broker.exe"
                source.parent.mkdir(parents=True)
                source.write_bytes(b"synthetic broker")
                mcp = source.with_name("loomward-mcp.exe")
                mcp.write_bytes(b"synthetic MCP server")
                with mock.patch.object(broker, "ROOT", root), mock.patch.object(broker.subprocess, "check_output", side_effect=[
                    json.dumps({"target_directory": str(target)}), "x86_64-pc-windows-msvc\n",
                ]), mock.patch.object(broker.subprocess, "run") as run:
                    broker.build(release)
                self.assertEqual((root / "native/binaries/loomward-broker-x86_64-pc-windows-msvc.exe").read_bytes(), source.read_bytes())
                staged_mcp = root / "native/binaries/loomward-mcp-x86_64-pc-windows-msvc.exe"
                self.assertTrue(staged_mcp.is_file(), "MCP sidecar was not staged")
                self.assertEqual(staged_mcp.read_bytes(), mcp.read_bytes())
                command = run.call_args.args[0]
                self.assertIn("--locked", command)
                self.assertIn("loomward-broker", command)
                self.assertIn("loomward-mcp", command)
                self.assertEqual(command[command.index("--target-dir") + 1], str(target))
                self.assertEqual("--release" in command, release)
                self.assertTrue(run.call_args.kwargs["check"])

    def test_failed_build_does_not_stage_a_stale_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "target/debug/loomward-broker.exe"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"stale broker")
            with mock.patch.object(broker, "ROOT", root), mock.patch.object(broker.subprocess, "check_output", side_effect=[
                json.dumps({"target_directory": str(root / "target")}), "x86_64-pc-windows-msvc\n",
            ]), mock.patch.object(broker.subprocess, "run", side_effect=subprocess.CalledProcessError(1, ["cargo", "build"])):
                with self.assertRaises(subprocess.CalledProcessError):
                    broker.build()
            self.assertFalse((root / "native/binaries").exists())


if __name__ == "__main__":
    unittest.main()
