#!/usr/bin/env python3
"""Build/run the linked offline prototype. Never opens USB or installs a driver.

Every Rust test gets its own process: intended faults can retain native threads.
Use thread for Rust+C TSan (nightly/rust-src), address for Rust ASan+C UBSan,
and native-address for C ASan/UBSan with ordinary Rust. Apple Clang and Rust
ship incompatible ASan version guards; never suppress or alias those guards.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess

ROOT = Path(__file__).resolve().parents[2]
PREFIX = "live::tests::integrated_tests::"
SLOW = ("phase_and_offset_screen", "dense_offsets_and_negative_controls", "delayed_callback_offsets", "cost_delay_and_hal_disturbance_screen")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT / "build/integrated-completion")
    parser.add_argument("--sanitizer", choices=("none", "address", "native-address", "thread"), default="none")
    parser.add_argument("--quick", action="store_true", help="omit four broad timing sweeps (run all without --quick)")
    parser.add_argument("--case", action="append", default=[], help="test-name substring (repeatable)")
    parser.add_argument("--jobs", type=int, default=1, choices=range(1, 5))
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    commands = []

    def command(argv, env=None):
        result = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True)
        commands.append(dict(argv=argv, returncode=result.returncode, stdout=result.stdout, stderr=result.stderr))
        (out / "build-commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        return result.stdout

    command(["rustc", "-vV"])
    command(["clang", "--version"])
    if args.sanitizer in ("address", "thread"):
        command(["rustc", "+nightly", "-vV"])
    flags = ["-std=c11", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-Wno-deprecated-declarations", "-fno-omit-frame-pointer", "-mmacosx-version-min=14.0"]
    if args.sanitizer != "none":
        flags += ["-fsanitize=" + {"address": "undefined", "native-address": "address,undefined", "thread": "thread"}[args.sanitizer]]
    objects = []
    for name in ("completion_route", "integration_native", "integration_allocations"):
        obj = out / (name + ".o")
        audit = ["-D" + call + "=ftu_audit_" + call for call in ("malloc", "calloc", "realloc", "free")] if name == "completion_route" else []
        command(["clang", *flags, *audit, "-c", str(ROOT / "tools/ftu-probe" / (name + ".c")), "-o", str(obj)])
        objects.append(str(obj))
    command(["ar", "rcs", str(out / "libftu_integrated.a"), *objects])
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(out / "target")
    env["MACOSX_DEPLOYMENT_TARGET"] = "14.0"
    rustflags = f"-Lnative={out} -lstatic=ftu_integrated -lframework=CoreFoundation -lframework=IOKit -Cforce-frame-pointers=yes"
    cargo = ["cargo"]
    if args.sanitizer in ("address", "thread"):
        cargo += ["+nightly"]
        rustflags += " -Zsanitizer=" + args.sanitizer
        if args.sanitizer == "address":
            # Clang's UBSan handlers supplement Rust's ASan runtime.
            rustflags += " -Clink-arg=-fsanitize=undefined"
    if args.sanitizer == "native-address":
        runtime = Path(command(["clang", "-print-resource-dir"]).strip()) / "lib/darwin"
        rustflags += f" -Clink-arg={runtime}/libclang_rt.asan_osx_dynamic.dylib -Clink-arg=-Wl,-rpath,{runtime}"
    if args.sanitizer != "none":
        env["ASAN_OPTIONS"] = "detect_leaks=0:abort_on_error=1"
        env["UBSAN_OPTIONS"] = "halt_on_error=1:print_stacktrace=1"
        env["TSAN_OPTIONS"] = "halt_on_error=1"
    env["RUSTFLAGS"] = rustflags
    cargo += ["test", "--release", "--locked", "--offline", "-p", "ftu-stream", "--features", "native-integration", "--no-run", "--message-format=json"]
    if args.sanitizer in ("address", "thread"):
        target = ("aarch64" if platform.machine() == "arm64" else "x86_64") + "-apple-darwin"
        cargo += ["-Zbuild-std", "--target", target]
    # Cargo does not track an archive supplied only through RUSTFLAGS. Remove
    # this crate's cached harness so every run links the C objects just compiled.
    clean = ["cargo", *(["+nightly"] if args.sanitizer in ("address", "thread") else []), "clean", "--release", "-p", "ftu-stream"]
    if args.sanitizer in ("address", "thread"):
        clean += ["--target", target]
    command(clean, env)
    output = command(cargo, env)
    artifacts = [json.loads(line) for line in output.splitlines() if line.startswith("{")]
    binaries = [a["executable"] for a in artifacts if a.get("reason") == "compiler-artifact" and a.get("profile", {}).get("test") and a.get("executable")]
    assert len(binaries) == 1, binaries
    binary = binaries[0]
    listing = command([binary, "--list"], env)
    cases = [line.removesuffix(": test") for line in listing.splitlines() if line.startswith(PREFIX) and line.endswith(": test")]
    if args.quick:
        cases = [c for c in cases if not c.endswith(SLOW)]
    if args.case:
        cases = [c for c in cases if any(s in c for s in args.case)]
    assert cases, "No linked tests selected"
    source_names = [str(p.relative_to(ROOT)) for folder in ("rust", "shared", "tools/ftu-probe") for p in (ROOT / folder).rglob("*") if p.is_file() and p.suffix in (".rs", ".toml", ".c", ".h", ".py", ".mk")] + ["Cargo.toml", "Cargo.lock"]
    (out / "tested-source-sha256.json").write_text(json.dumps({n: hashlib.sha256((ROOT / n).read_bytes()).hexdigest() for n in source_names}, indent=2) + "\n")

    def run(case):
        argv = [binary, "--exact", case, "--nocapture", "--test-threads=1"]
        result = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800)
        log = result.stdout + result.stderr
        name = case.removeprefix(PREFIX) + ".log"
        (out / name).write_text(log)
        clean = not any(s in log for s in ("ERROR: AddressSanitizer", "WARNING: ThreadSanitizer", "runtime error:"))
        passed = result.returncode == 0 and "test result: ok. 1 passed;" in log and clean
        print(("PASS " if passed else "FAIL ") + case, flush=True)
        return dict(case=case, argv=argv, returncode=result.returncode, passed=passed, log=name)

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(run, cases))
    (out / "results.json").write_text(json.dumps(dict(sanitizer=args.sanitizer, rust_sanitizer={"none": None, "address": "address", "native-address": None, "thread": "thread"}[args.sanitizer], c_sanitizer={"none": None, "address": "undefined", "native-address": "address,undefined", "thread": "thread"}[args.sanitizer], tests=results), indent=2) + "\n")
    assert all(r["passed"] for r in results), "See per-case logs"
    print(f"PASS: {len(results)} linked native/Rust cases ({args.sanitizer})")


if __name__ == "__main__":
    main()
