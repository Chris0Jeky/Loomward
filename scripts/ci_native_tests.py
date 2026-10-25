"""Stream native test output and report elevation coverage without masking Cargo failures."""
import os
from pathlib import Path
import subprocess
import sys

MARKER = "SKIPPED (elevated): "


def main(command=None):
    command = sys.argv[1:] if command is None else command
    sys.stdout.reconfigure(errors="replace")
    skipped = 0
    with subprocess.Popen(
        command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, encoding="utf-8", errors="replace",
    ) as process:
        for line in process.stdout:
            print(line, end="", flush=True)
            skipped += line.count(MARKER)
        status = process.wait()
    summary = (
        f"### Native test coverage: `{' '.join(command)}`\n\n"
        f"Cargo exit status: {status}.\n\n"
        f"Elevated-skip markers observed: **{skipped}** (repeated checks count separately).\n\n"
    )
    if skipped:
        summary += (
            "UNVERIFIED: these tests returned early or exercised only elevation refusal. "
            "Their unelevated native behavior was not exercised; Cargo may still report them as passed.\n\n"
        )
    else:
        summary += "No elevated-skip markers observed; this does not prove complete native coverage.\n\n"
    print(summary, flush=True)
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(path).open("a", encoding="utf-8") as output:
            output.write(summary)
    return status


if __name__ == "__main__":
    raise SystemExit(main())
