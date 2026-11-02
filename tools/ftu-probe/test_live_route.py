#!/usr/bin/env python3
"""Offline actual duplex attach/dispose boundary, with injected USB interfaces."""
from pathlib import Path
import argparse
import json
import os
import subprocess

ROOT = Path(__file__).resolve().parents[2]
CASES = ("normal", "abort", "attach-failure", "policy-failure", "cleanup-failure",
         "removed", "not-attached", "removed-not-open", "removed-raw-destroy",
         "removed-cleanup-error", "removed-one", "not-open-without-removal",
         "raw-error-without-removal", "removed-contradict-alt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT / "build/live-route")
    parser.add_argument("--sanitizer", choices=("address,undefined", "thread"), default="address,undefined")
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, ASAN_OPTIONS="detect_leaks=0:abort_on_error=1",
               UBSAN_OPTIONS="halt_on_error=1:print_stacktrace=1", TSAN_OPTIONS="halt_on_error=1")
    commands = []

    def run(argv):
        p = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=120)
        commands.append(dict(argv=[str(x) for x in argv], returncode=p.returncode, stdout=p.stdout, stderr=p.stderr))
        (out / "results.json").write_text(json.dumps(commands, indent=2) + "\n")
        if p.returncode:
            raise RuntimeError(p.stdout + p.stderr)

    run(["cargo", "build", "--release", "--locked", "--offline", "-p", "ftu-stream"])
    binary = out / "test-live-route"
    run(["clang", "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror",
         "-Wno-deprecated-declarations", "-fsanitize=" + args.sanitizer, "-fno-omit-frame-pointer",
         "-mmacosx-version-min=14.0", "tools/ftu-probe/test_live_route.c",
         "tools/ftu-probe/completion_route.c", "target/release/libftu_stream.a",
         "-framework", "CoreAudio", "-framework", "CoreFoundation", "-framework", "IOKit",
         "-framework", "Security", "-liconv", "-o", binary])
    for case in CASES:
        run([binary, case])
        print("PASS " + case, flush=True)
    print(f"PASS: {len(CASES)} actual native adapter cases ({args.sanitizer})")


if __name__ == "__main__":
    main()
