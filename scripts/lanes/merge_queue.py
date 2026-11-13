#!/usr/bin/env python3
"""Serial, reviewed-SHA merge queue. Run with --once and/or --dry-run to inspect."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


def decide(facts):
    """Return (action, reason) from collected facts, without I/O or a clock."""
    if facts["state"] in {"MERGED", "CLOSED"}:
        return "done", facts["state"].lower()
    if facts["state"] != "OPEN":
        return "wait", "PR state unknown"
    commits = facts["commits"]
    if (not facts["reviewed_ancestor"]
            or (facts["head"] != facts["reviewed"] and not commits)
            or any(c["parent_count"] != 2 or not c["second_parent_on_main"]
                   or not c["tree_matches"] for c in commits)):
        return "park", "head has unreviewed commits"
    if facts["merge_state"] == "DIRTY":
        return "park", "merge conflicts"
    if facts["behind"] is None or facts["merge_state"] in {"", "UNKNOWN"}:
        return "wait", "mergeability unknown"
    if facts["behind"] > 0:
        return "update", "head is behind main"
    pending = not facts["checks"]
    cancelled = False
    for check in facts["checks"]:
        conclusion = check.get("conclusion") or check.get("state", "")
        status = check.get("status", "COMPLETED" if "state" in check else "")
        if status != "COMPLETED" or conclusion in {"", "EXPECTED", "PENDING"}:
            pending = True
        elif conclusion == "CANCELLED":
            cancelled = True
        elif conclusion not in {"SUCCESS", "SKIPPED", "NEUTRAL"}:
            return "park", "CI failed"
    if cancelled:
        return "rerun", "CI cancelled"
    if pending:
        return "wait", "CI pending or absent"
    if not any((c.get("conclusion") or c.get("state")) == "SUCCESS" for c in facts["checks"]):
        return "park", "no successful check (estate_merge treats all-skipped CI as absent)"
    if facts["observed_seconds"] < 180:
        return "wait", "head observed for less than 180 s"
    return "merge", "reviewed head, current main, green CI, observed for at least 180 s"


def run(repo, *args, check=True, env=None, timeout=120):
    result = subprocess.run(args, cwd=repo, text=True, encoding="utf-8",
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout, env=env)
    if check and result.returncode:
        # estate_merge.py prints its refusal reason as JSON on stdout; keep it in the log line.
        detail = (result.stdout.strip() or result.stderr.strip())[-400:]
        raise RuntimeError(f"{' '.join(args[:2])} exited {result.returncode}: {detail}")
    return result


def ancestor(repo, older, newer):
    result = run(repo, "git", "merge-base", "--is-ancestor", older, newer, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError("git ancestry unavailable")
    return result.returncode == 0


def commit_facts(repo, reviewed, head):
    """Prove the first-parent path; imported main history is not new lane work."""
    if not ancestor(repo, reviewed, head):
        return False, []
    commits = []
    current = head
    while current != reviewed:
        tree, *parents = run(repo, "git", "show", "-s", "--format=%T %P", current).stdout.split()
        commit = {"parent_count": len(parents), "second_parent_on_main": False, "tree_matches": False}
        commits.append(commit)
        if len(parents) != 2:
            break
        commit["second_parent_on_main"] = ancestor(repo, parents[1], "origin/main")
        merged = run(repo, "git", "merge-tree", "--write-tree", *parents, check=False)
        commit["tree_matches"] = merged.returncode == 0 and merged.stdout.strip() == tree
        if not commit["second_parent_on_main"] or not commit["tree_matches"]:
            break
        current = parents[0]
    return True, commits


def gather(repo, slug, reviewed, info):
    def gh_json(*args):
        return json.loads(run(repo, "gh", *args).stdout)

    facts = {"state": info["state"]}
    if info["state"] != "OPEN":
        return facts
    if info["baseRefName"] != "main" or info["isDraft"]:
        raise RuntimeError("queue requires a ready PR targeting main")
    head = info["headRefOid"]
    run(repo, "git", "fetch", "origin")
    for sha in (reviewed, head):
        if run(repo, "git", "cat-file", "-e", f"{sha}^{{commit}}", check=False).returncode:
            run(repo, "git", "fetch", "origin", sha)
    related, commits = commit_facts(repo, reviewed, head)
    compare = gh_json("api", f"repos/{slug}/compare/main...{head}")
    facts.update(head=head, reviewed=reviewed, reviewed_ancestor=related, commits=commits,
                 merge_state=info["mergeStateStatus"], behind=compare["behind_by"],
                 checks=info["statusCheckRollup"] or [])
    return facts


def observe(seen, pr, head, now):
    """Return a new observation and its age; never use a commit timestamp."""
    previous = seen.get(pr, {})
    if previous.get("head") != head:
        previous = {"head": head, "first_seen": now}
    return previous, max(0, now - previous["first_seen"])


def log(pr, action, reason, dry_run):
    stamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    print(f"{stamp} #{pr} {'dry-run ' if dry_run else ''}{action}: {reason}", flush=True)


def pop(queue, first):
    # Re-read after network work so lines appended during a pass survive.
    with queue.open("r+", encoding="utf-8") as stream:
        lines = stream.readlines()
        if not lines or lines[0] != first:
            raise RuntimeError("queue head changed during pass")
        stream.seek(0)
        stream.writelines(lines[1:])
        stream.truncate()


def one_pass(repo, home, slug, dry_run):
    queue = home / "mergeq.txt"
    first = ""
    pr = "-"
    try:
        if queue.exists():
            with queue.open(encoding="utf-8") as stream:
                first = stream.readline()
        if not first:
            log(pr, "wait", "queue empty", dry_run)
            return
        entry = re.fullmatch(r"([1-9][0-9]*)\s+([0-9a-fA-F]{40})(?:\s+(\S+))?", first.strip())
        lane = None
        if entry:
            pr, reviewed, lane = entry.groups()
        if not entry:
            action, reason = "park", "expected <PR> <40-hex reviewed SHA> [<lane>]"
            reviewed = "?"
        elif lane is not None and not re.fullmatch(r"[A-Za-z0-9_-]+", lane):
            action, reason = "park", "invalid lane: expected [A-Za-z0-9_-]+"
        else:
            reviewed = reviewed.lower()
            seen_file = home / "mergeq.seen.json"
            seen = json.loads(seen_file.read_text(encoding="utf-8")) if seen_file.exists() else {}
            info = json.loads(run(repo, "gh", "pr", "view", pr, "--repo", slug, "--json",
                                  "state,mergeStateStatus,headRefOid,statusCheckRollup,baseRefName,isDraft").stdout)
            age = 0
            if info["state"] == "OPEN":
                head = info["headRefOid"]
                if not re.fullmatch(r"[0-9a-f]{40}", head):
                    raise ValueError("invalid remote head SHA")
                # When a head was first seen is an observation, not an effect: dry runs record it too.
                observation, age = observe(seen, pr, head, time.time())
                seen[pr] = observation
                temporary = seen_file.with_suffix(".json.tmp")
                temporary.write_text(json.dumps(seen) + "\n", encoding="utf-8")
                temporary.replace(seen_file)
            facts = gather(repo, slug, reviewed, info)
            facts["observed_seconds"] = age
            action, reason = decide(facts)
            if action == "merge":
                # Narrow the race with a moving main: re-compare just before the call. estate_merge.py
                # re-checks CI and pins the head itself, but not the base (GitHub offers no atomic check).
                again = json.loads(run(repo, "gh", "api", f"repos/{slug}/compare/main...{facts['head']}").stdout)
                if again["behind_by"]:
                    action, reason = "update", "main moved during the pass"
        if action == "merge":
            args = ["py", "-3", str(Path.home() / ".claude/tools/estate_merge.py"), slug, pr,
                    "--head", facts["head"], "--actor", "agent:claude-opus-5-5", "--runtime", "claude"]
            if dry_run:
                args.append("--dry-run")
            run(repo, *args)
            if not dry_run:
                pop(queue, first)
            if lane is not None:
                # Pin PATH's Bash: Windows' process search can otherwise select WSL's bash.exe.
                teardown = [shutil.which("bash") or "bash",
                            str(Path(repo) / "scripts/lanes/rmwt.sh"), lane]
                if dry_run:
                    log(pr, "teardown", f"would run {subprocess.list2cmdline(teardown)}", True)
                else:
                    # The merge is already consumed: a teardown refusal must never retry it.
                    try:
                        # Removing a multi-GB target/ can outlast the 120 s gh timeout.
                        result = run(Path.cwd(), *teardown, check=False, timeout=900,
                                     env={**os.environ, "LW_HOME": str(home)})
                        output = (result.stdout + "\n" + result.stderr).strip().splitlines()
                        last = output[-1] if output else "(no output)"
                        log(pr, "teardown", f"{lane} exited {result.returncode}: {last}", False)
                    except (OSError, subprocess.SubprocessError) as exc:
                        log(pr, "teardown", f"{lane} failed: {' '.join(str(exc).splitlines())}", False)
        elif not dry_run:
            if action == "park":
                with (home / "mergeq.parked").open("a", encoding="utf-8") as parked:
                    parked.write(f"{first.strip()} {reason}\n")
                pop(queue, first)
            elif action == "done":
                pop(queue, first)
            elif action == "update":
                run(repo, "gh", "pr", "update-branch", pr, "--repo", slug)
            elif action == "rerun":
                runs = run(repo, "gh", "api", "--paginate",
                           f"repos/{slug}/actions/runs?head_sha={facts['head']}&status=cancelled&per_page=100",
                           "--jq", ".workflow_runs[].id").stdout.split()
                for workflow in runs:
                    run(repo, "gh", "run", "rerun", workflow, "--repo", slug)
        log(pr, action, reason, dry_run)
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.SubprocessError) as exc:
        log(pr, "wait", " ".join(str(exc).splitlines()), dry_run)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--once", action="store_true", help="run one pass")
    parser.add_argument("--dry-run", action="store_true", help="decide and print; record observed heads; call the merge helper only with --dry-run; change nothing else")
    args = parser.parse_args()
    repo = run(Path(__file__).resolve().parent, "git", "rev-parse", "--show-toplevel").stdout.strip()
    home = Path(os.environ.get("LW_HOME") or "C:/lw")
    slug = os.environ.get("LW_REPO_SLUG") or "Chris0Jeky/Loomward"
    while True:
        one_pass(repo, home, slug, args.dry_run)
        if args.once:
            break
        time.sleep(240)


if __name__ == "__main__":
    main()
