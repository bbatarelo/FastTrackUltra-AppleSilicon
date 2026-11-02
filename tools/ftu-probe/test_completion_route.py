"""Each intentional quarantine has process lifetime, never a fake cleanup override."""
import subprocess
import sys

CASES = ("baseline", "ten", "geometry", "stress", "early", "reject", "early-error",
         "abort", "removed", "not-attached", "removed-lost", "removed-poison", "lost-abort", "abort-error", "hold", "lost-release", "stalled-consumer",
         "wrong-thread", "wrong-frame", "duplicate", "bad-release", "stale",
         "post-destroy", "cross-session", "stop-admission", "reject-late", "token-exhaustion", "invalid", "source-v0", "zero-timeout", "late-abort", "dead-source", "dispose-error", "lost-message", "timer-failure", "thread-failure")

if __name__ == "__main__":
    for case in CASES:
        result = subprocess.run([sys.argv[1], case], text=True, capture_output=True, timeout=30)
        print(case + ": " + result.stdout.strip(), flush=True)
        if result.returncode:
            print(result.stderr, file=sys.stderr)
            raise SystemExit(result.returncode)
    print(f"PASS: {len(CASES)} native routing/lifecycle cases", flush=True)
