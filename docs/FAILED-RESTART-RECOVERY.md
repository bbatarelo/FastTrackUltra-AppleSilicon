# Recovery after a clean failed restart

Incident on 2 November; offline fix prepared 3 November 2026. This candidate fixes a permanent refusal after a recoverable
restart failed before reaching RUNNING. Hardware qualification is pending.
The ordinary `make install` profile remains the 8 ms/four-slot baseline;
this policy is enabled only in the experimental 4 ms/twelve-slot driver.

## Observed failure

The installed `dev-fdb892cf3bcabfec` was still attached to the correct Ultra
USB enumeration. A reason-55 output-deadline fault in epoch 197 triggered
recovery. The replacement worker, epoch 198, encountered another deadline
failure before its first output request: 13 ms remained against the unchanged
16 ms requirement. The recorded gap between successful and failing owner steps
was about 33 ms. These observations do not identify what delayed the worker.

All 16 native callbacks returned, were released and acknowledged. The dispatcher
joined, Rust slots retired, the route was destroyed, both interfaces were restored
and closed, and factory close succeeded. There was no uncertain ownership or
quarantine. Nevertheless, recovery settled as `refused / attempt_failed` after
one consecutive attempt, despite a budget of three. One admitted client remained.

Quitting/reopening Spotify and switching both default output selections to the
Mac's speakers and back did not release that client or change the epoch. The
current diagnostics do not identify which application owns that HAL client.
The user subsequently restarted CoreAudio and confirmed audio returned.
Evidence: `build/trials/2026-11-02-spotify-recovery-fault/`.

The 31 October incident is distinct: removal and rebind succeeded, then initial
capture failed with reason 23, no output requests and no retained clients.
Its current completion's raw error was not retained; the lifetime-first record
belongs to epoch 1. This change does not claim to fix that USB completion error.
Evidence: `build/trials/2026-10-31-reconnect-startup-fault/`.

## Changed behavior

After a recovery launch fails, Rust may schedule its remaining attempts only if:

- the launch returned the existing failure code `-5` and opened a newer epoch;
- the two-second startup wait did not expire (a late deadline reported during
  cancellation cannot turn a startup timeout into an eligible retry);
- that worker has joined, no HAL calls remain admitted, and the phase is FAULT;
- a coherent, finished runtime record belongs to that new epoch;
- the existing classifier proves a clean output-deadline failure: reason 55,
  an OUT guard at stages 3–6, fault 6, successful drain/cleanup/factory close.

The failed launch is counted as **failed**, never as recovered. Its newly
classified fault replaces the recovery diagnostic's fault epoch. The state
becomes `pending` with the next backoff, or `exhausted` if no attempts remain.
The call that just failed still returns `-5`; later polls return waiting or
attempt again. The C monitor continues polling FAULT even after publishing
`DeviceIsRunning=false`; a boundary regression covers `-5`, waiting, then
success without a client Start. The diagnostic's `last_status=-5` alongside `state=pending`
therefore means a failed attempt has a scheduled retry, not that audio is running.

Each subsequent control poll rechecks attachment/binding, clients, admitted IO,
worker completion and fault evidence. A stale binding, removal or replacement
cannot open a different USB enumeration. Last-client Stop and detach cancel
pending retries. Unknown Stops do not cancel another client's recovery.
Neither a new client nor a retained client's repeated Start can bypass the wait
or restore the budget. Recovery requires no fresh Start from the admitted clients.

The budget remains **three consecutive attempts**, with **0/1/4-second backoff**
and a **30-second healthy run** before reset. Failed starts consume that same
budget. They do not create independent retry budgets. A persistent failure
exhausts the budget and remains stopped. First client starts without an admitted
client, unknown USB completion errors, stale/unfinished fault records, setup
failures, thread-policy failures, failed cleanup/close and quarantine do not
become eligible. There is no reset-quarantine operation.

Transport scheduling, sample placement, buffers, safety offsets, minimum leads,
watchdog, PCM-readiness policy and callback ownership rules are unchanged.
No new work is added to realtime HAL IO. Successful recovery uses a new clock
epoch; an interrupted stream still fails uninterrupted-audio qualification.

## What E-MU contributes

Read-only comparison used `emu-tracker-pre` commit
`5921d33cec07f80c4f8e0e4c158e0f65c795779f`; the three examined files had no local
changes. No E-MU code or installed driver was modified.

- `driver/usb_engine.c`, `engine_thread`: after an established stream faults,
  failed rebuilds remain inside a bounded retry loop. An initial client start
  that never came up is handled separately. This distinction directly informed
  the FTU change: a clean failed recovery launch should not discard its budget.
- The same engine uses six attempts, exponential backoff and five seconds of
  healthy operation to reset its budget. FTU retains its own three attempts,
  0/1/4-second delays and 30-second reset; those numbers are not copied.
- `scripts/test-recovery.sh` deliberately makes the first rebuild fail before
  a later rebuild succeeds. It checks actual `framesBound` growth and no further
  unmapped frames, not just recovery counters. FTU's new linked test likewise
  resumes HAL writes/reads and requires zero steady missing frames after warm-up.
- E-MU keeps its mid-stream timeline continuous and logs faults/rebuilds. On
  exhausted recovery, its failure handler in `driver/plugin.c` explicitly
  publishes `DeviceIsAlive=false`. FTU uses fresh timestamp epochs and still
  needs a separate persistent-failure availability/logging review. Copying
  E-MU's timestamp handling without its sample-mapping design would be unsafe.
- E-MU also handles a stale setup schedule by rebuilding from a fresh bus frame.
  This FTU change rebuilds only after the existing full retirement proof; it
  does not reschedule outstanding requests or weaken deadline acceptance.

## Offline acceptance and next hardware check

The new linked test reproduces the observed 33 ms startup gap and 13 ms remaining
OUT lead. It fails against the unchanged policy at `refused` versus `pending`.
With the patch, the original two clients remain registered, the failed epoch
publishes no running timestamp, the next attempt obeys the production one-second
backoff, and a new epoch resumes input/output with zero steady missing frames.

Additional tests cover repeated failed startups exhausting exactly three
attempts; failed counters without false recoveries; Stop/detach cancellation;
unknown Stops; stale/replaced binding; outstanding HAL calls; and refusal after
cleanup, close or thread-policy failures. The actual two-second startup timeout is also tested: a deadline arriving during
cancellation must remain refused. Mixed client-Start/monitor retries share the
budget, exhaustion cannot be bypassed, and an ordinary baseline build refuses
automatic recovery even with an otherwise eligible deadline record.

Validation completed offline:

| Check | Result |
| --- | --- |
| Regression against the original policy | Failed at sticky refusal, as expected |
| Full linked native/Rust suite | 56/56 passed |
| Full Rust + C ThreadSanitizer suite | 56/56 passed |
| Full Rust AddressSanitizer + C UndefinedBehaviorSanitizer suite | 56/56 passed |
| Baseline `make test` | 217 Rust tests and 120 Python tests passed; three Rust tests intentionally ignored |
| Final baseline policy and disabled-profile checks | 7/7 passed |
| Baseline and independent HAL boundary, native hotplug and session guard | Passed |
| C monitor continued polling after failed reopen | Passed under C ASan/UBSan for both profiles |
| Formatting, baseline/independent Clippy, both driver builds | Passed |

The full baseline run preceded the timeout-provenance refinement; the final
baseline policy/disabled-profile checks, both HAL boundaries and builds cover
that refinement. The final 56-case matrices include the timeout regression.
After those matrices only a moved doc comment and additional counter assertions
changed Rust; the affected assertions were rerun. Per-run source hashes are
retained so these stages are distinguishable.

Claude reviewed the change twice. Its timeout concern was addressed and its
monitor-loop integration question is covered by the C boundary regression.
Conservative refusals remain possible if control-thread preemption carries the
startup beyond its two-second deadline, or an already-entered HAL call remains
admitted at classification; neither case permits unsafe reopening. An attempt
still holds the existing control lock during bounded startup/cleanup. Tests do
not model every host reaction to the stopped-engine notification or a worker
that briefly reaches RUNNING and faults again before launch observes it.

Install the separately prepared candidate, verify its loaded identity and run
silence preflight followed by the unchanged loopback/listening checks. Then
qualify retained-client recovery and longer playback. Offline fake USB/clock
results are not proof of physical Spotify resumption. A new driver may reduce
this failure's impact, but the underlying scheduling delay and the distinct
initial capture completion error remain open reliability investigations.


## Frozen candidate

- Build: `dev-215622171af39021` (4 ms/twelve-slot experimental profile).
- Executable SHA-256: `a5d21baa1ba4571efc045a86ba8189045dc866057f718ccf355640616c94ac91`.
- Candidate: `build/candidates/failed-restart-recovery-20261103/`.
- Validation, review disposition and tested source: `build/validation/failed-restart-recovery-20261103/`.
- Rollback is the preserved, currently installed `dev-fdb892cf3bcabfec` bundle.
- No installation, CoreAudio restart or commit was performed while preparing this fix.

From the repository root, install the frozen candidate with:

```sh
/bin/bash build/validation/failed-restart-recovery-20261103/install.sh
```

This checks its hash/signature, preserves the installed bundle, asks for the Mac
administrator password and restarts CoreAudio. Use the sibling `rollback.sh`
to restore the preserved driver. Either script accepts `--check` to validate
without installation. `make install` continues to select the baseline profile;
use the candidate script for this experiment.
