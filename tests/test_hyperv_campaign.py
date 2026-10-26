"""No host access: the PowerShell boundary is entirely scripted."""

import base64
import ast
import contextlib
import copy
import io
import json
import re
import sys
import unittest
from unittest.mock import patch

from scripts.hyperv_campaign import (
    Campaign, CampaignError, Config, LAB_ROOT, PowerShell, encoded, main, window_for_label,
)


class FakeTime:
    def __init__(self):
        self.now = 0.0
        self.sleeps = []

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds


class FakePowerShell:
    # These are deliberately never interpolated into generated scripts.
    guest_json = {"runner": "FAKE-runner-password-91", "admin": "FAKE-admin-password-73"}

    def __init__(self):
        self.calls = []
        self.records = []
        self.responses = {}
        self.preflight = {
            "module": True, "free_bytes": 100 * 1024**3,
            "vm": {"name": "LoomwardCrashLab", "checkpoint_type": "Disabled",
                   "automatic_checkpoints": False, "network_adapters": 0,
                   "paths": [{"raw": LAB_ROOT + r"\disks\lab.vhdx",
                              "resolved": LAB_ROOT + r"\disks\lab.vhdx"}]},
        }

    def run(self, script, timeout):
        operation = script.splitlines()[0].split(":", 1)[1].strip()
        self.calls.append((operation, script, timeout))
        if operation == "append":
            payload = re.search(r"FromBase64String\('([^']+)'\)", script).group(1)
            self.records.append(json.loads(base64.b64decode(payload)))
        defaults = {
            "preflight": self.preflight, "next_cycle": {"cycle": 1},
            "boot": {"heartbeat": "OkApplicationsHealthy"},
            "cache": {"enabled": True},
            "prepare": {"exit_code": 0, "runner_output": "prepared"},
            "act": {"started": True},
            "poll": {"trace_lines": 4, "last_trace_label": "item_written",
                     "dirty_pages": 128, "act_done": False, "elevated_token": False},
            "health": {"chkdsk": {"code": 0, "clean": True, "summary": "No problems"},
                       "write_cache": {"enabled": True}},
            "recover": {"result": {"ok": True, "recovered": "open", "abandoned": "done",
                                   "journal_records": 4, "last_record": "item_verified",
                                   "copy_ran": True, "failure": None},
                        "last_trace_label": "item_written", "free_bytes": 4 * 1024**3,
                        "runner_output": "recovered", "exit_code": 0},
        }
        response = self.responses.get(operation, defaults.get(operation, {}))
        if isinstance(response, list):
            response = response.pop(0)
        if isinstance(response, Exception):
            raise response
        return json.dumps(copy.deepcopy(response))


class CampaignTests(unittest.TestCase):
    def make(self, **kwargs):
        import random
        self.ps = FakePowerShell()
        self.time = FakeTime()
        self.config = Config(execute=True, seed=19, **kwargs)
        return Campaign(self.ps, random.Random(19), self.time.clock, self.time.sleep, self.config)

    def operations(self):
        return [op for op, _, _ in self.ps.calls]

    def test_dry_run_all_commands_never_access_system_or_passwords(self):
        for command in ("preflight", "cycle", "campaign", "negative"):
            with self.subTest(command=command):
                output = io.StringIO()
                fake = FakePowerShell()
                args = [command, "--seed", "19"]
                if command == "negative":
                    args += ["--exe", r"C:\lw\mutant.exe"]
                with contextlib.redirect_stdout(output), patch(
                    "scripts.hyperv_campaign.PowerShell", return_value=fake
                ), patch("scripts.hyperv_campaign.time.sleep", side_effect=AssertionError("sleep")):
                    self.assertEqual(main(args), 0)
                self.assertFalse(fake.calls)
                text = output.getvalue()
                self.assertIn("Get-Command Get-VM", text)
                for password in fake.guest_json.values():
                    self.assertNotIn(password, text)
                self.assertNotIn(LAB_ROOT + r"\state", text)
                if command != "preflight":
                    self.assertIn("<redacted>", text)
                    self.assertLess(text.index("# lw-hv:prepare"), text.index("# lw-hv:act"))
                    self.assertIn("Stop-VM -Name", text)
                    self.assertIn("-TurnOff -Force", text)
                    self.assertIn("Copy-Item -FromSession", text)

    def test_preflight_refusals(self):
        cases = [
            ("C drive", "path_outside_allowlist", "path", r"C:\lab.vhdx"),
            ("H drive", "path_outside_allowlist", "path", r"H:\lab.vhdx"),
            ("resolved junction", "path_outside_allowlist", "path", r"C:\junction-target\lab.vhdx"),
            ("prefix sibling", "path_outside_allowlist", "path", LAB_ROOT + r"-other\lab.vhdx"),
            ("dot escape", "path_outside_allowlist", "path", LAB_ROOT + r"\..\lab.vhdx"),
            ("checkpoint", "checkpoints_enabled", "checkpoint_type", "Production"),
            ("automatic checkpoint", "checkpoints_enabled", "automatic_checkpoints", True),
            ("network", "network_adapter_present", "network_adapters", 1),
            ("foreign", "foreign_vm", "name", "SomeOtherVM"),
            ("space", "host_free_space", "free_bytes", 79 * 1024**3),
            ("module", "hyperv_module_unavailable", "module", False),
        ]
        for name, reason, key, value in cases:
            with self.subTest(name=name):
                c = self.make()
                if key == "path":
                    self.ps.preflight["vm"]["paths"][0]["resolved"] = value
                elif key in ("module", "free_bytes"):
                    self.ps.preflight[key] = value
                else:
                    self.ps.preflight["vm"][key] = value
                with self.assertRaisesRegex(CampaignError, reason):
                    c.preflight()
                self.assertEqual(self.operations(), ["preflight"])

    def test_preflight_absent_vm_and_normalised_path(self):
        c = self.make()
        self.ps.preflight["vm"]["paths"][0]["resolved"] = r"e:/LOOMWARD-LAB/hyperv/disks/lab.vhdx"
        c.preflight()
        self.ps.preflight["vm"] = None
        c.preflight()
        with self.assertRaisesRegex(CampaignError, "vm_absent"):
            c.cycle(1)
        self.assertNotIn("boot", self.operations())

    def test_floor_cycle_record_and_scripts(self):
        c = self.make()
        record = c.cycle(7)
        self.assertEqual(self.ps.records, [record])
        self.assertEqual(record["cycle"], 7)
        self.assertEqual(record["kind"], "floor")
        self.assertEqual(record["floor_credit"], "write")
        self.assertEqual(record["last_record"], "item_verified")
        self.assertEqual(record["last_trace_label"], "item_written")
        self.assertEqual(record["dirty_pages"], 128)
        self.assertTrue(record["chkdsk"]["clean"])
        self.assertIn("total_s", record["timings"])
        self.assertGreaterEqual(record["delay_ms"], 0)
        self.assertLessEqual(record["delay_ms"], 2000)
        scripts = {op: s for op, s, _ in self.ps.calls}
        self.assertIn("Start-Process", scripts["act"])
        self.assertIn("-WindowStyle Hidden", scripts["act"])
        self.assertIn(".cmd", scripts["act"])
        self.assertIn("LOOMWARD_HV_STEP=act", scripts["act"])
        self.assertIn("c00007", scripts["prepare"])
        self.assertIn("'lwrunner'", scripts["prepare"])
        self.assertIn("'lwadmin'", scripts["health"])
        self.assertNotIn("'lwadmin'", scripts["recover"])
        self.assertIn("chkdsk L: /scan", scripts["health"])
        self.assertEqual(self.operations().count("poll"), 2)

    def test_post_completion_detected_during_delay_gets_no_credit(self):
        c = self.make()
        initial = {"trace_lines": 3, "dirty_pages": 128, "act_done": False}
        final = {"trace_lines": 18, "dirty_pages": 32, "act_done": True}
        self.ps.responses["poll"] = [initial, final]
        record = c.cycle(1)
        self.assertEqual(record["kind"], "post_completion")
        self.assertIsNone(record["floor_credit"])
        self.assertEqual(record["trace_lines"], 18)
        self.assertEqual(record["dirty_pages"], 32)
        self.assertIn("cut", self.operations())

    def test_zero_dirty_pages_gets_no_credit(self):
        c = self.make()
        self.ps.responses["poll"] = {"trace_lines": 4, "dirty_pages": 0, "act_done": False}
        self.assertIsNone(c.cycle(1)["floor_credit"])

    def test_durable_completion_marker_catches_the_final_poll_cut_race(self):
        c = self.make()
        response = json.loads(self.ps.run("# lw-hv:recover", 1))
        self.ps.calls.clear()
        response["act_done"] = True
        self.ps.responses["recover"] = response
        record = c.cycle(1)
        self.assertEqual(record["kind"], "post_completion")
        self.assertIsNone(record["floor_credit"])

    def test_waits_until_more_than_two_trace_lines(self):
        c = self.make()
        self.ps.responses["poll"] = [
            {"trace_lines": 2, "dirty_pages": 1, "act_done": False},
            {"trace_lines": 3, "dirty_pages": 1, "act_done": False},
            {"trace_lines": 3, "dirty_pages": 1, "act_done": False},
        ]
        c.cycle(1)
        self.assertIn(0.25, self.time.sleeps)

    def test_failure_stops_campaign_and_preserves_evidence(self):
        for defect in ("oracle", "elevated", "copy", "chkdsk", "heartbeat", "cache"):
            with self.subTest(defect=defect):
                c = self.make(command="campaign", cycles=3)
                if defect in ("oracle", "copy"):
                    response = json.loads(self.ps.run("# lw-hv:recover", 1))
                    self.ps.calls.clear()
                    response["result"]["ok" if defect == "oracle" else "copy_ran"] = False
                    self.ps.responses["recover"] = response
                elif defect == "elevated":
                    self.ps.responses["prepare"] = {"exit_code": 0, "runner_output": "ERROR: elevated-token detected"}
                elif defect == "chkdsk":
                    self.ps.responses["health"] = {"chkdsk": {"code": 3, "clean": False},
                                                   "write_cache": {"enabled": True}}
                elif defect == "cache":
                    self.ps.responses["cache"] = {"enabled": False}
                else:
                    self.ps.responses["boot"] = {"heartbeat": "NoContact"}
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(CampaignError):
                    c.run()
                self.assertLessEqual(self.operations().count("prepare"), 1)
                self.assertNotIn("reset", self.operations())
                self.assertTrue(self.ps.records)
                self.assertFalse(self.ps.records[-1]["ok"])
                self.assertEqual(c.summary["cycles"], 1)
                self.assertEqual(c.summary["failures"], 1)
                if defect == "heartbeat":
                    self.assertGreaterEqual(self.time.now, 300)
                    self.assertEqual(self.ps.records[-1]["failure"], "guest_boot_failed")
                    self.assertNotIn("cut", self.operations())

    def test_negative_continues_only_oracle_failures(self):
        c = self.make(command="negative", cycles=3, exe=r"C:\lw\mutant.exe")
        response = json.loads(self.ps.run("# lw-hv:recover", 1))
        self.ps.calls.clear()
        response["result"]["ok"] = False
        response["result"]["failure"] = "mutant caught"
        response["exit_code"] = 101
        self.ps.responses["recover"] = response
        with contextlib.redirect_stdout(io.StringIO()):
            summary = c.run()
        self.assertEqual(summary["cycles"], 3)
        self.assertEqual(summary["caught"], 3)
        self.assertEqual(summary["failures"], 3)
        self.assertEqual(len(self.ps.records), 3)
        self.assertTrue(all(r["floor_credit"] is None for r in self.ps.records))
        self.assertIn("mutant.exe", next(s for op, s, _ in self.ps.calls if op == "act"))

    def test_negative_still_stops_on_missing_copy_or_elevation(self):
        for defect in ("copy", "elevation"):
            with self.subTest(defect=defect):
                c = self.make(command="negative", cycles=3, exe=r"C:\lw\mutant.exe")
                response = json.loads(self.ps.run("# lw-hv:recover", 1))
                self.ps.calls.clear()
                response["result"]["ok"] = False
                if defect == "copy":
                    response["result"]["copy_ran"] = False
                else:
                    response["runner_output"] = "elevated token; refusing"
                self.ps.responses["recover"] = response
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(CampaignError):
                    c.run()
                self.assertEqual(self.operations().count("prepare"), 1)
                self.assertEqual(len(self.ps.records), 1)

    def test_poll_detects_elevation_and_preparation_exit_failure(self):
        for operation, response in (
            ("poll", {"trace_lines": 4, "dirty_pages": 1, "act_done": False, "elevated_token": True}),
            ("prepare", {"exit_code": 101, "runner_output": "ordinary test failure"}),
        ):
            with self.subTest(operation=operation):
                c = self.make()
                self.ps.responses[operation] = response
                with self.assertRaises(CampaignError):
                    c.cycle(1)
                self.assertNotIn("cut", self.operations())

    def test_powershell_failure_and_invalid_response_refuse_without_vm_cleanup(self):
        for response in (CampaignError("synthetic I/O failure"), None):
            with self.subTest(response=response):
                c = self.make()
                self.ps.responses["poll"] = response
                with self.assertRaises(CampaignError):
                    c.cycle(1)
                self.assertNotIn("cut", self.operations())
                self.assertNotIn("reset", self.operations())

    def test_seed_repeatability_and_abandon_forces_removal(self):
        first = self.make(scenario="abandon", removal="0").cycle(1)
        second = self.make(scenario="abandon", removal="0").cycle(1)
        self.assertEqual(first["delay_ms"], second["delay_ms"])
        self.assertEqual(first["removal"], 1)

    def test_config_rejects_cmd_expansion_and_nonpositive_cycles(self):
        for path in (r"C:\lw\%bad%.exe", 'C:\\lw\\bad".exe', "relative.exe", "C:\\lw\\bad\n.exe"):
            with self.subTest(path=path), self.assertRaises(CampaignError):
                self.make(exe=path)
        with self.assertRaises(CampaignError):
            self.make(cycles=0)

    @unittest.skipUnless(sys.platform == "win32", "Windows PowerShell parser")
    def test_generated_powershell_syntax_only_never_invokes_generated_scripts(self):
        # The campaign uses the fake. The real PowerShell invocation ONLY parses
        # strings as ASTs; none of the Hyper-V, credential or guest code is run.
        c = self.make()
        c.cycle(50)
        scripts = [script for _, script, _ in self.ps.calls]
        parser = """
$scripts = ConvertFrom-Json PAYLOAD
$problems = @()
foreach ($script in $scripts) {
    $tokens = $null; $errors = $null
    [System.Management.Automation.Language.Parser]::ParseInput(
        $script, [ref]$tokens, [ref]$errors) | Out-Null
    foreach ($problem in $errors) { $problems += $problem.Message }
}
@{ problems = @($problems) } | ConvertTo-Json -Compress

""".replace("PAYLOAD", ("(" + encoded(json.dumps(scripts)) + ")"))
        result = json.loads(PowerShell().run(parser, 30))
        self.assertEqual(result["problems"], [])

    def test_python_311_grammar(self):
        # Parse source under the minimum grammar without importing other runtimes.
        import inspect
        import scripts.hyperv_campaign as module
        ast.parse(inspect.getsource(module), feature_version=(3, 11))

    def test_reset_at_fifty_and_low_space(self):
        for cycles, low, expected in ((51, False, 1), (2, True, 2)):
            with self.subTest(cycles=cycles, low=low):
                c = self.make(command="campaign", cycles=cycles, scenario="mixed", removal="mixed")
                if low:
                    response = json.loads(self.ps.run("# lw-hv:recover", 1))
                    self.ps.calls.clear()
                    response["free_bytes"] = 1024**3
                    self.ps.responses["recover"] = response
                with contextlib.redirect_stdout(io.StringIO()):
                    summary = c.run()
                self.assertEqual(summary["cycles"], cycles)
                self.assertEqual(self.operations().count("reset"), expected)
                for record in self.ps.records:
                    if record["cycle"] % 2 == 0:
                        self.assertEqual((record["scenario"], record["removal"]), ("abandon", 1))
                script = next(s for op, s, _ in self.ps.calls if op == "reset")
                self.assertIn("pristine-lab.vhdx", script)
                self.assertIn("pristine-signal.vhdx", script)
                self.assertIn("Stop-VM -Name", script)
                self.assertNotIn("-TurnOff", script)
                self.assertIn(".Length", script)
                self.assertIn("pristine_disk_size_unexpected", script)

    def test_append_failure_is_not_retried(self):
        c = self.make()
        self.ps.responses["append"] = CampaignError("append I/O failure")
        with self.assertRaisesRegex(CampaignError, "append I/O failure"):
            c.cycle(1)
        self.assertEqual(self.operations().count("append"), 1)

    def test_bounded_act_timeout_and_unknown_dirty_pages(self):
        c = self.make()
        self.ps.responses["poll"] = {"trace_lines": 0, "dirty_pages": 1, "act_done": False}
        with self.assertRaisesRegex(CampaignError, "act_progress_timeout"):
            c.cycle(1)
        self.assertNotIn("cut", self.operations())
        c = self.make()
        self.ps.responses["poll"] = {"trace_lines": 4, "dirty_pages": None, "act_done": False}
        with self.assertRaisesRegex(CampaignError, "dirty_pages_unavailable"):
            c.cycle(1)

    def test_label_mapping(self):
        table = {
            "create": "create_batch", "identities_recorded": "create_batch",
            "create_batch": "create_batch", "item_written": "write", "verified": "write",
            "chunk_flushed": "write", "publish_intent": "publish", "renamed": "publish",
            "committed": "publish", "item_removed": "cleanup", "staging_removed": "cleanup",
            "cancelling": "cleanup", "preparing": "other", "": "other", None: "other",
        }
        for label, window in table.items():
            with self.subTest(label=label):
                self.assertEqual(window_for_label(label), window)

    def test_powershell_uses_stdin_and_propagates_stderr(self):
        with patch("scripts.hyperv_campaign.subprocess.run") as run:
            run.return_value.returncode = 0
            run.return_value.stdout = "{}"
            self.assertEqual(PowerShell().run("# harmless fake", 12), "{}")
            self.assertEqual(run.call_args.args[0],
                             ["powershell", "-NoProfile", "-NonInteractive", "-Command", "-"])
            self.assertEqual(run.call_args.kwargs["input"], "# harmless fake")
            self.assertEqual(run.call_args.kwargs["timeout"], 12)
            run.return_value.returncode = 1
            run.return_value.stderr = "synthetic failure"
            with self.assertRaisesRegex(CampaignError, "synthetic failure"):
                PowerShell().run("# harmless fake", 12)


if __name__ == "__main__":
    unittest.main()
