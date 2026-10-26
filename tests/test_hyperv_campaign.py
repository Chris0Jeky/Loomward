"""No Hyper-V access: fake runner, syntax parsing and a disposable path-guard lab."""

import base64
import ast
import contextlib
import copy
import io
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path
import unittest
import uuid
from unittest.mock import patch

from scripts.hyperv_campaign import (
    Campaign, CampaignError, Config, LAB_ROOT, PATH_GUARD, PowerShell, encoded, main, quote, window_for_label,
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
            "cut": {"trace_lines": 4, "dirty_pages": 128, "act_done": False,
                    "elevated_token": False, "cut_latency_ms": 1400.125,
                    "last_poll_return": "2026-10-26T12:00:00.0000000Z",
                    "stop_started": "2026-10-26T12:00:01.3000000Z",
                    "stop_returned": "2026-10-26T12:00:01.4001250Z"},
            "health": {"chkdsk": {"code": 0, "clean": True, "summary": "No problems"},
                       "filesystem": {"dirty": False, "dirty_query": "Volume L: is NOT Dirty",
                                      "repair_events": []},
                       "write_cache": {"enabled": True}},
            "recover": {"result": {"ok": True, "recovered": "open", "abandoned": "done",
                                   "journal_records": 4, "last_record": "item_verified",
                                   "copy_ran": True, "failure": None,
                                   "terminal_before_recover": False},
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
                ), patch("scripts.hyperv_campaign.time.sleep", side_effect=AssertionError("sleep")), patch(
                    "scripts.hyperv_campaign.subprocess.run", side_effect=AssertionError("subprocess")
                ):
                    self.assertEqual(main(args), 0)
                self.assertFalse(fake.calls)
                text = output.getvalue()
                self.assertIn("Get-Command Get-VM", text)
                for password in fake.guest_json.values():
                    self.assertNotIn(password, text)
                self.assertNotIn(LAB_ROOT + r"\state", text)
                if command != "preflight":
                    self.assertIn("<redacted>", text)
                    self.assertLess(text.index("# lw-hv:prepare"), text.index("# Act wrapper"))
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
        self.assertEqual(record["anchor"], "progress")
        self.assertEqual(record["floor_credit"], "write")
        self.assertEqual(record["last_record"], "item_verified")
        self.assertEqual(record["last_trace_label"], "item_written")
        self.assertEqual(record["dirty_pages"], 128)
        self.assertTrue(record["chkdsk"]["clean"])
        self.assertIn("total_s", record["timings"])
        self.assertGreaterEqual(record["delay_ms"], 0)
        self.assertLessEqual(record["delay_ms"], 2000)
        scripts = {op: s for op, s, _ in self.ps.calls}
        cut = scripts["cut"]
        self.assertNotIn("act", scripts)
        # The act starts in the cut's own session, before the poll and the hard-off: a process
        # started through PowerShell Direct ends when its session closes.
        self.assertEqual(cut.count("New-PSSession"), 1)
        self.assertLess(cut.index("Start-Process"), cut.index("$deadline"))
        self.assertLess(cut.index("Start-Process"), cut.index("-TurnOff"))
        self.assertIn("-WindowStyle Hidden", cut)
        self.assertIn(".cmd", cut)
        self.assertIn("LOOMWARD_HV_STEP=act", cut)
        self.assertIn("c00007", scripts["prepare"])
        self.assertIn("'lwrunner'", scripts["prepare"])
        self.assertIn("'lwadmin'", scripts["health"])
        self.assertNotIn("'lwadmin'", scripts["recover"])
        self.assertIn("chkdsk L: /scan", scripts["health"])
        self.assertEqual(self.operations().count("cut"), 1)
        self.assertNotIn("poll", self.operations())
        self.assertEqual(record["seed"], 19)
        self.assertEqual(record["cut_latency_ms"], 1400.125)
        self.assertEqual(record["stop_returned"], "2026-10-26T12:00:01.4001250Z")
        cut = scripts["cut"]
        self.assertLess(cut.index("New-PSSession"), cut.index("$poll = Invoke-Command -Session"))
        self.assertLess(cut.index("$lastPollReturn = Get-Date"), cut.index("Start-Sleep -Milliseconds $delay"))
        self.assertLess(cut.index("Start-Sleep -Milliseconds $delay"), cut.index("Stop-VM"))
        self.assertIn("($stopReturned - $lastPollReturn).TotalMilliseconds", cut)
        self.assertEqual(self.time.sleeps, [])

    def test_post_completion_detected_during_delay_gets_no_credit(self):
        c = self.make()
        final = json.loads(self.ps.run("# lw-hv:cut", 1))
        self.ps.calls.clear()
        final.update(trace_lines=18, dirty_pages=32, act_done=True)
        self.ps.responses["cut"] = final
        record = c.cycle(1)
        self.assertEqual(record["kind"], "post_completion")
        self.assertIsNone(record["floor_credit"])
        self.assertEqual(record["trace_lines"], 18)
        self.assertEqual(record["dirty_pages"], 32)
        self.assertIn("cut", self.operations())

    def test_launch_anchor_negative_cut_and_completion(self):
        for completion in (None, "marker", "terminal"):
            with self.subTest(completion=completion):
                c = self.make(command="negative", anchor="launch", exe=r"C:\lw\mutant.exe")
                self.ps.responses["cut"] = {
                    "trace_lines": None, "dirty_pages": None, "act_done": None,
                    "delay_ms": 88, "cut_latency_ms": 98.5, "last_poll_return": None,
                    "launch_returned": "2026-10-26T12:00:00.0000000Z",
                    "stop_started": "2026-10-26T12:00:00.0880000Z",
                    "stop_returned": "2026-10-26T12:00:00.0985000Z",
                }
                recovered = json.loads(self.ps.run("# lw-hv:recover", 1))
                self.ps.calls.clear()
                recovered["act_done"] = completion == "marker"
                recovered["result"]["terminal_before_recover"] = completion == "terminal"
                self.ps.responses["recover"] = recovered
                record = c.cycle(1)
                self.assertEqual(record["anchor"], "launch")
                self.assertEqual(record["kind"], "negative_control" if completion is None else "post_completion")
                self.assertIsNone(record["floor_credit"])
                self.assertEqual(sum(c.summary["floor_credits"].values()), 0)
                for key in ("trace_lines", "dirty_pages", "last_poll_return"):
                    self.assertIsNone(record[key])
                self.assertEqual(record["launch_returned"], "2026-10-26T12:00:00.0000000Z")
                self.assertEqual(record["cut_latency_ms"], 98.5)
                self.assertEqual(self.ps.records, [record])
                cut = next(s for op, s, _ in self.ps.calls if op == "cut")
                after_launch = cut.split(c._launch, 1)[1]
                before_stop = after_launch.split("Stop-VM", 1)[0]
                self.assertNotIn("Invoke-Command", before_stop)
                self.assertNotIn("Get-Counter", after_launch)
                self.assertLess(before_stop.index("$launchReturned = Get-Date"),
                                before_stop.index("Start-Sleep -Milliseconds $delay"))
                self.assertIn("$delay = " + str(record["delay_ms"]), before_stop)
                self.assertIn("-TurnOff -Force", after_launch)
                self.assertIn("($stopReturned - $launchReturned).TotalMilliseconds", after_launch)
                for field in ("trace_lines", "dirty_pages", "act_done", "last_poll_return"):
                    self.assertIn(field + " = $null", after_launch)
                for field, variable in (("launch_returned", "launchReturned"),
                                        ("stop_started", "stopStarted"), ("stop_returned", "stopReturned")):
                    self.assertIn(f"{field} = ${variable}.ToString('o')", after_launch)

    def test_parser_launch_anchor_is_negative_only(self):
        for command in ("campaign", "cycle", "preflight"):
            with self.subTest(command=command), patch("scripts.hyperv_campaign.Campaign") as campaign:
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                    main([command, "--anchor", "launch"])
                self.assertEqual(error.exception.code, 2)
                campaign.assert_not_called()
        with patch("scripts.hyperv_campaign.Campaign") as campaign:
            self.assertEqual(main(["negative", "--exe", r"C:\lw\mutant.exe",
                                   "--anchor", "launch", "--keep-going", "2"]), 0)
            config = campaign.call_args.args[-1]
            self.assertEqual((config.anchor, config.keep_going), ("launch", 2))

    def test_keep_going_bounds_consecutive_infrastructure_failures(self):
        cases = (
            (1, [True, False], False, 2, 1, 1),
            (1, [True, True, False], True, 2, 2, 1),
            (1, [True, False, True, False], False, 4, 2, 2),
            (0, [True, False], True, 1, 1, 0),
            (1, [True], False, 1, 1, 0),
        )
        for budget, failures, stops, cycles, infra, offs in cases:
            with self.subTest(budget=budget, failures=failures):
                c = self.make(command="campaign", cycles=len(failures), keep_going=budget)
                boot = []
                for failed in failures:
                    boot.extend([CampaignError("powershell_unavailable_or_timeout")] if failed else
                                [{"heartbeat": "Ok"}, {"heartbeat": "Ok"}])
                self.ps.responses["boot"] = boot
                with contextlib.redirect_stdout(io.StringIO()):
                    if stops:
                        with self.assertRaisesRegex(CampaignError, "boot:"):
                            c.run()
                    else:
                        c.run()
                self.assertEqual(c.summary["cycles"], cycles)
                self.assertEqual(c.summary["failures"], infra)
                self.assertEqual(c.summary["infrastructure_failures"], infra)
                self.assertEqual([r["cycle"] for r in self.ps.records], list(range(1, cycles + 1)))
                self.assertTrue(all(r["anchor"] == "progress" for r in self.ps.records))
                operations = self.operations()
                self.assertEqual(operations.count("ensure_off"), offs)
                for index, operation in enumerate(operations):
                    if operation == "ensure_off":
                        self.assertEqual(operations[index - 1:index + 3],
                                         ["append", "ensure_off", "preflight", "start"])
                for operation, script, _ in self.ps.calls:
                    if operation == "ensure_off":
                        self.assertIn("(Get-VM -Name 'LoomwardCrashLab').State -ne 'Off'", script)
                        self.assertIn("Stop-VM -Name 'LoomwardCrashLab' -TurnOff -Force", script)
                        self.assertIn("function Resolve-LabPath", script)

    def test_keep_going_start_and_cache_failures(self):
        for operation, success in (("start", {}), ("cache", {"enabled": True})):
            with self.subTest(operation=operation):
                c = self.make(command="campaign", cycles=2, keep_going=1)
                self.ps.responses[operation] = [CampaignError("synthetic failure"), success, success]
                with contextlib.redirect_stdout(io.StringIO()):
                    summary = c.run()
                self.assertEqual((summary["cycles"], summary["failures"], summary["infrastructure_failures"]),
                                 (2, 1, 1))
                self.assertEqual(self.ps.records[0]["failure"], operation + ":synthetic failure")
                self.assertEqual(self.operations().count("ensure_off"), 1)

    def test_keep_going_exact_infrastructure_failures(self):
        for failure in ("guest_boot_failed", "powershell_unavailable_or_timeout", "guest_direct_connection_timeout"):
            with self.subTest(failure=failure):
                c = self.make(command="campaign", cycles=2, keep_going=1)
                with patch.object(c, "_boot", side_effect=[CampaignError(failure), None, None]), \
                        contextlib.redirect_stdout(io.StringIO()):
                    summary = c.run()
                self.assertEqual(self.ps.records[0]["failure"], failure)
                self.assertEqual((summary["cycles"], summary["infrastructure_failures"]), (2, 1))

    def test_keep_going_does_not_match_failure_substrings_or_absent_vm(self):
        for failure in ("guest_boot_failed_extra", "prepare:guest_direct_connection_timeout"):
            with self.subTest(failure=failure):
                c = self.make(command="campaign", cycles=2, keep_going=5)
                with patch.object(c, "_boot", side_effect=CampaignError(failure)), \
                        contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(CampaignError, failure):
                    c.run()
                self.assertEqual(c.summary["infrastructure_failures"], 0)
                self.assertNotIn("ensure_off", self.operations())
        c = self.make(command="campaign", cycles=2, keep_going=5)
        self.ps.preflight["vm"] = None
        with self.assertRaisesRegex(CampaignError, "vm_absent"):
            c.run()
        self.assertEqual(self.operations(), ["preflight"])

    def test_keep_going_never_retries_safety_or_oracle_failures(self):
        for reason in ("oracle_failed", "filesystem_repaired", "write_cache_not_enabled",
                       "chkdsk_dirty_or_failed", "copy_did_not_run", "elevated_token",
                       "prepare_failed", "lab_free_space_unavailable", "invalid_guest_result"):
            with self.subTest(reason=reason):
                c = self.make(command="campaign", cycles=3, keep_going=5)
                if reason in ("oracle_failed", "copy_did_not_run", "invalid_guest_result",
                              "lab_free_space_unavailable"):
                    response = json.loads(self.ps.run("# lw-hv:recover", 1))
                    if reason == "oracle_failed":
                        response["result"]["ok"] = False
                    elif reason == "copy_did_not_run":
                        response["result"]["copy_ran"] = False
                    elif reason == "invalid_guest_result":
                        response["result"] = None
                    else:
                        response["free_bytes"] = None
                    self.ps.responses["recover"] = response
                elif reason in ("filesystem_repaired", "chkdsk_dirty_or_failed"):
                    health = json.loads(self.ps.run("# lw-hv:health", 1))
                    if reason == "filesystem_repaired":
                        health["filesystem"]["dirty"] = True
                    else:
                        health["chkdsk"]["clean"] = False
                    self.ps.responses["health"] = health
                elif reason == "write_cache_not_enabled":
                    self.ps.responses["cache"] = {"enabled": False}
                else:
                    self.ps.responses["prepare"] = {"exit_code": 1, "elevated_token": reason == "elevated_token"}
                self.ps.calls.clear()
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(CampaignError, reason):
                    c.run()
                self.assertEqual(c.summary["infrastructure_failures"], 0)
                self.assertEqual(self.ps.records[-1]["failure"], reason)
                self.assertEqual(c.summary["cycles"], 1)
                self.assertNotIn("ensure_off", self.operations())

    def test_keep_going_stops_if_ensure_off_fails(self):
        c = self.make(command="campaign", cycles=2, keep_going=1)
        self.ps.responses.update(boot=CampaignError("synthetic boot failure"),
                                 ensure_off=CampaignError("synthetic turn-off failure"))
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(CampaignError, "ensure_off:"):
            c.run()
        self.assertEqual(self.operations().count("start"), 1)
        self.assertEqual(c.summary["cycles"], 1)
        script = next(s for op, s, _ in self.ps.calls if op == "ensure_off")
        # A refused Stop-VM is tolerated; only a VM still not Off after the deadline fails.
        self.assertIn("catch {", script)
        self.assertIn("throw 'vm_not_off'", script)

    def test_zero_dirty_pages_gets_no_credit(self):
        c = self.make()
        response = json.loads(self.ps.run("# lw-hv:cut", 1))
        self.ps.calls.clear()
        response["dirty_pages"] = 0
        self.ps.responses["cut"] = response
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
        c.cycle(1)
        script = next(s for op, s, _ in self.ps.calls if op == "cut")
        self.assertIn("while ($true)", script)
        self.assertIn("$poll.trace_lines -gt 2 -or $poll.act_done", script)
        self.assertIn("Start-Sleep -Milliseconds 250", script)

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

    def test_every_session_proves_its_guest_identity(self):
        # PowerShell Direct can return the previous session's identity (measured 2026-10-26).
        c = self.make()
        c.cycle(1)
        scripts = {op: s for op, s, _ in self.ps.calls}
        for op, account in (("prepare", "lwrunner"), ("recover", "lwrunner"), ("cut", "lwrunner"),
                            ("health", "lwadmin")):
            with self.subTest(op=op):
                self.assertIn("WindowsIdentity]::GetCurrent().Name", scripts[op])
                self.assertIn(f"-like ('*\\' + '{account}')", scripts[op])
                self.assertIn("guest_session_identity_mismatch", scripts[op])

    def test_negative_cycles_never_earn_floor_credit(self):
        c = self.make(command="negative", cycles=1, exe=r"C:\lw\mutant.exe")
        with contextlib.redirect_stdout(io.StringIO()):
            c.run()
        record = self.ps.records[-1]
        self.assertTrue(record["ok"])
        self.assertEqual(record["command"], "negative")
        self.assertEqual(record["exe"], r"C:\lw\mutant.exe")
        self.assertIsNone(record["floor_credit"])
        self.assertEqual(sum(c.summary["floor_credits"].values()), 0)

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
        self.assertIn("mutant.exe", next(s for op, s, _ in self.ps.calls if op == "cut"))

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
            ("cut", {"trace_lines": 4, "dirty_pages": 1, "act_done": False, "elevated_token": True}),
            ("prepare", {"exit_code": 101, "runner_output": "ordinary test failure"}),
        ):
            with self.subTest(operation=operation):
                c = self.make()
                self.ps.responses[operation] = response
                with self.assertRaises(CampaignError):
                    c.cycle(1)
                self.assertNotIn("recover", self.operations())

    def test_powershell_failure_and_invalid_response_refuse_without_vm_cleanup(self):
        for response in (CampaignError("synthetic I/O failure"), None):
            with self.subTest(response=response):
                c = self.make()
                self.ps.responses["cut"] = response
                with self.assertRaises(CampaignError):
                    c.cycle(1)
                self.assertNotIn("recover", self.operations())
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
        for options in ({"anchor": "launch"}, {"anchor": "unknown"}, {"keep_going": -1}):
            with self.subTest(options=options), self.assertRaisesRegex(CampaignError, "invalid_configuration"):
                self.make(**options)
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
            main(["campaign", "--keep-going", "-1"])
        self.assertEqual(error.exception.code, 2)

    @unittest.skipUnless(sys.platform == "win32" and shutil.which("powershell"), "Windows PowerShell parser")
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
        self.ps.responses["cut"] = CampaignError("act_progress_timeout")
        with self.assertRaisesRegex(CampaignError, "act_progress_timeout"):
            c.cycle(1)
        self.assertNotIn("recover", self.operations())
        c = self.make()
        self.ps.responses["cut"] = {"trace_lines": 4, "dirty_pages": None, "act_done": False}
        with self.assertRaisesRegex(CampaignError, "dirty_pages_unavailable"):
            c.cycle(1)

    def test_label_mapping(self):
        table = {
            "create": "create_batch", "identities_recorded": "create_batch",
            "create_batch": "create_batch", "planned": "write", "item_written": "write", "verified": "write",
            "chunk_flushed": "write", "publish_intent": "publish", "renamed": "publish",
            "committed": "publish", "item_removed": "cleanup", "staging_removed": "cleanup",
            "cancelling": "cleanup", "preparing": "other", "": "other", None: "other",
        }
        for label, window in table.items():
            with self.subTest(label=label):
                self.assertEqual(window_for_label(label), window)

    def test_terminal_journal_before_recovery_excludes_floor_without_marker(self):
        for terminal, expected in ((True, "post_completion"), (False, "floor")):
            with self.subTest(terminal=terminal):
                c = self.make()
                response = json.loads(self.ps.run("# lw-hv:recover", 1))
                self.ps.calls.clear()
                response["result"]["terminal_before_recover"] = terminal
                self.ps.responses["recover"] = response
                record = c.cycle(1)
                self.assertEqual(record["kind"], expected)
                if terminal:
                    self.assertIsNone(record["floor_credit"])

    def test_missing_terminal_field_is_never_guessed(self):
        c = self.make()
        response = json.loads(self.ps.run("# lw-hv:recover", 1))
        self.ps.calls.clear()
        del response["result"]["terminal_before_recover"]
        response["result"]["last_record"] = "transition:committed"
        self.ps.responses["recover"] = response
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(CampaignError, "invalid_guest_result"):
            c.cycle(1)

    def test_filesystem_repair_evidence_stops_before_recover_and_is_logged(self):
        for dirty, events in ((True, []), (False, [{"Id": 55, "ProviderName": "Ntfs"}])):
            with self.subTest(dirty=dirty):
                c = self.make(command="campaign", cycles=3)
                health = json.loads(self.ps.run("# lw-hv:health", 1))
                self.ps.calls.clear()
                health["filesystem"].update(dirty=dirty, repair_events=events)
                self.ps.responses["health"] = health
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(
                    CampaignError, "filesystem_repaired"
                ):
                    c.run()
                self.assertEqual(self.ps.records[-1]["filesystem"], health["filesystem"])
                self.assertEqual(self.ps.records[-1]["failure"], "filesystem_repaired")
                self.assertNotIn("recover", self.operations())
                self.assertNotIn("reset", self.operations())
                script = next(s for op, s, _ in self.ps.calls if op == "health")
                self.assertLess(script.index("fsutil dirty query L:"), script.index("chkdsk L: /scan"))
                self.assertLess(script.index("Get-WinEvent"), script.index("chkdsk L: /scan"))
                for text in ("LastBootUpTime", "'Application','System'", "Wininit", "Chkdsk",
                             "Microsoft-Windows-Ntfs", "$filesystem.dirty"):
                    self.assertIn(text, script)

    @unittest.skipUnless(sys.platform == "win32" and shutil.which("powershell"), "Windows PowerShell required")
    def test_filesystem_prescan_runs_with_fake_guest_commands(self):
        c = self.make()
        c._admin(health=True)
        script = self.ps.calls[-1][1]
        # Execute only the pre-scan fragment, with every guest command replaced.
        # No session, VM cmdlet, counter or physical-disk query is executed.
        check = script[script.index("$dirtyQuery ="):script.index("if (-not ('LwCacheQuery'")]
        cases = [
            {"dirty": True, "events": [], "scan": False, "repairs": 0},
            {"dirty": False, "events": [], "scan": True, "repairs": 0},
        ]
        for provider, event_id, level, message, repairs in (
            ("Ntfs", 98, 4, "Volume L: is healthy. No action is needed.", 0),
            ("Ntfs", 55, 4, "File system corruption", 1),
            ("Microsoft-Windows-Ntfs", 140, 3, "Flush failed", 1),
            ("Wininit", 1001, 4, "Windows has made corrections to the file system", 1),
            ("Chkdsk", 26226, 4, "No problems found", 0),
            ("Other", 55, 2, "Unrelated warning", 0),
        ):
            cases.append({"dirty": False, "events": [{"ProviderName": provider, "Id": event_id,
                "Level": level, "Message": message}], "scan": not repairs, "repairs": repairs})
        harness = r"""
$ErrorActionPreference = 'Stop'
function fsutil { $global:LASTEXITCODE = 0; if ($case.dirty) { 'Volume L: is Dirty' } else { 'Volume L: is NOT Dirty' } }
function chkdsk { $script:scanned = $true; $global:LASTEXITCODE = 0; 'No problems' }
function Get-CimInstance { @{ LastBootUpTime = [DateTime]'2026-10-26T00:00:00Z' } }
function Get-WinEvent { param($FilterHashtable, $ErrorAction, $ErrorVariable) $case.events }
$answers = @(foreach ($case in (ConvertFrom-Json PAYLOAD)) {
    $script:scanned = $false
    CHECK
    @{ dirty = $filesystem.dirty; repairs = @($filesystem.repair_events).Count; scan = $script:scanned }
})
@{ answers = $answers } | ConvertTo-Json -Depth 5 -Compress

""".replace("PAYLOAD", "(" + encoded(json.dumps(cases)) + ")").replace("CHECK", check)
        answers = json.loads(PowerShell().run(harness, 30))["answers"]
        self.assertEqual(answers, [{key: case[key] for key in ("dirty", "scan", "repairs")} for case in cases])

    def test_connection_retries_cover_both_guest_accounts(self):
        c = self.make()
        c.cycle(1)
        for op, script, timeout in self.ps.calls:
            if op in ("cache", "health", "prepare", "act", "cut", "recover"):
                with self.subTest(operation=op):
                    self.assertIn("AddSeconds(120)", script)
                    self.assertIn("New-PSSession -VMName", script)
                    self.assertIn("Start-Sleep -Seconds 1", script)
                    self.assertIn("guest_direct_connection_timeout", script)
                    self.assertGreater(timeout, 120)

    def test_failure_logging_cannot_mask_original_error(self):
        c = self.make()
        self.ps.responses["prepare"] = CampaignError("original failure")
        self.ps.responses["append"] = CampaignError("logging failure")
        with self.assertRaisesRegex(CampaignError, "original failure"):
            c.cycle(1)
        self.assertEqual(self.operations().count("append"), 1)
        self.assertEqual(c.summary["failures"], 1)

    def test_random_seed_default_and_explicit_seed(self):
        with patch("scripts.hyperv_campaign.secrets.randbits", return_value=12345) as randbits:
            config = Config()
            self.assertEqual(config.seed, 12345)
            randbits.assert_called_once_with(32)
        with patch("scripts.hyperv_campaign.secrets.randbits", return_value=54321) as randbits, patch(
            "scripts.hyperv_campaign.Campaign"
        ) as campaign:
            self.assertEqual(main(["preflight"]), 0)
            self.assertEqual(campaign.call_args.args[-1].seed, 54321)
            randbits.assert_called_once_with(32)
            randbits.reset_mock()
            self.assertEqual(main(["preflight", "--seed", "19"]), 0)
            self.assertEqual(campaign.call_args.args[-1].seed, 19)
            randbits.assert_not_called()

    def test_negative_not_caught_exits_one(self):
        output = io.StringIO()
        with patch("scripts.hyperv_campaign.PowerShell", return_value=FakePowerShell()), contextlib.redirect_stdout(output):
            self.assertEqual(main(["negative", "--execute", "--exe", r"C:\lw\mutant.exe", "--seed", "19"]), 1)
        self.assertIn("NOT CAUGHT", output.getvalue())
        with patch("scripts.hyperv_campaign.Campaign") as campaign:
            campaign.return_value.run.return_value = {"caught": 1}
            self.assertEqual(main(["negative", "--execute", "--exe", r"C:\lw\mutant.exe", "--seed", "19"]), 0)

    def test_reset_stages_and_hashes_before_replacing_either_disk(self):
        c = self.make()
        c._reset()
        script = next(s for op, s, _ in self.ps.calls if op == "reset")
        self.assertNotIn("$matches", script.lower())
        self.assertIn("pristine.json", script)
        self.assertIn("$pair.target + '.tmp'", script)
        self.assertIn("Get-FileHash -LiteralPath $temporary -Algorithm SHA256", script)
        self.assertIn("reset_hash_mismatch", script)
        self.assertLess(script.index("reset_hash_mismatch"), script.index("[IO.File]::Replace"))
        self.assertIn("$manifest", script)
        self.ps.responses["reset"] = CampaignError("reset_hash_mismatch")
        with self.assertRaisesRegex(CampaignError, "reset_hash_mismatch"):
            c._reset()
        c = self.make(command="campaign", cycles=3)
        response = json.loads(self.ps.run("# lw-hv:recover", 1))
        self.ps.calls.clear()
        response["free_bytes"] = 1024**3
        self.ps.responses.update(recover=response, reset=CampaignError("reset_hash_mismatch"))
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaisesRegex(CampaignError, "reset_hash_mismatch"):
            c.run()
        self.assertEqual(self.operations().count("prepare"), 1)

    def test_build_script_pristine_and_credential_hygiene(self):
        script = (Path(__file__).resolve().parents[1] / "scripts" / "hyperv_build_vm.ps1").read_text(encoding="utf-8")
        pristine = script.split("    'pristine' {", 1)[1]
        self.assertLess(pristine.index("pristine_already_exists_use_force"), pristine.index("Stop-VM"))
        self.assertIn("-not $Force", pristine)
        self.assertIn("-Filter 'pristine-*'", pristine)
        self.assertIn("Get-FileHash -LiteralPath $destination -Algorithm SHA256", pristine)
        self.assertIn("size = (Get-Item -LiteralPath $destination).Length", pristine)
        self.assertIn("'pristine.json'", pristine)
        unattend = script.split("    'unattend' {", 1)[1].split("    'create' {", 1)[0]
        self.assertIn("try {", unattend)
        self.assertIn("} finally {", unattend)
        self.assertIn("Remove-Item -LiteralPath $stage -Recurse -Force", unattend)
        # Container inheritance flags on the folder only: on a file they make icacls fail after
        # inheritance was removed, leaving an empty DACL (measured 2026-10-26).
        self.assertIn('icacls $State /inheritance:r /grant:r "${env:USERNAME}:(OI)(CI)F"', script)
        self.assertIn("""icacls $guest /inheritance:r /grant:r "${env:USERNAME}:F" 'SYSTEM:F'""", script)
        self.assertLess(script.index("iso_outside_allowlist"), script.index("New-Item"))
        self.assertIn("[IO.Path]::GetFullPath($Iso)", script)
        self.assertIn("$Root + '\\'", script)
        install = script.split("    'install' {", 1)[1].split("    'disks' {", 1)[0]
        self.assertLess(install.index("Remove-VMDvdDrive"), install.index("Remove-Item -LiteralPath"))
        self.assertIn("'iso\\unattend.iso'", install)
        self.assertIn(r"C:\ProgramData\Microsoft\Windows\Hyper-V", script)

    @unittest.skipUnless(sys.platform == "win32" and shutil.which("powershell"), "Windows PowerShell required")
    def test_path_guard_executes_against_junction_and_escapes(self):
        root = Path(LAB_ROOT)
        if not root.is_dir():
            self.skipTest("E:\\loomward-lab\\hyperv does not exist")
        folder = root / ("test-guard-" + uuid.uuid4().hex)
        folder.mkdir()
        junction = folder / "junction"
        try:
            # cmd expands its own TEMP; never read or print the host environment.
            result = subprocess.run(
                f'cmd /d /c mklink /J "{junction}" "%TEMP%"',
                capture_output=True, text=True, timeout=15, check=False,
            )
            self.assertEqual(result.returncode, 0, "junction creation failed")
            script = "$ErrorActionPreference = 'Stop'\n. {\n" + PATH_GUARD + "\n}\n"
            script += """
function Refused([string]$path) {
    try { Resolve-LabPath $path | Out-Null; return $false } catch { return $true }
}
"""
            for path in (str(junction / "x"), str(folder) + r"\..\..\..\escape", LAB_ROOT + r"-other\x"):
                script += f"if (-not (Refused {quote(path)})) {{ throw 'guard_accepted_escape' }}\n"
            script += f"Resolve-LabPath {quote(str(folder / 'plain-child'))} | Out-Null\n'{{}}'\n\n"
            self.assertEqual(json.loads(PowerShell().run(script, 30)), {})
        finally:
            # Remove the junction itself before traversing the disposable folder.
            if junction.is_junction():
                junction.rmdir()
            shutil.rmtree(folder)

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
