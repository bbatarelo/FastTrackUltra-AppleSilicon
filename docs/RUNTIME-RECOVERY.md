# Runtime fault record and bounded automatic recovery

**2 November policy update:** a failed recovery launch with a new, finished,
cleanly closed OUT deadline record now uses its remaining retry budget. See
[the incident, implementation and E-MU comparison](FAILED-RESTART-RECOVERY.md).
Other failed launches remain terminal for that session. Hardware validation of
this update is pending.

Status: **offline checkpoint completed 28 October; installation verified 30 October**.
Controlled hardware qualification remains incomplete.
The 30 October read-only investigation verified this candidate installed and
loaded: 13 automatic recoveries are recorded, followed by a separate
[disconnect/teardown quarantine](DISCONNECT-QUARANTINE-20261030.md). This is
field evidence of restarts, not a controlled audio/recovery qualification.
The original offline work responds to the
[28 October runtime stop](RUNTIME-STOP-20261028.md): epoch 48 cleanly stopped with
fault 6 / reason 55 (`output_lead_deadline`). The previous driver had no
automatic restart; retained clients could also refuse a new Start (-2). Client
membership was not exposed in the incident snapshot, so that particular
condition was not measured during the read-only investigation.

Two changes, both scoped to that incident:

1. **ftuE** — a fixed-size last-runtime-fault record naming the exact guard that
   stopped the owner, with the failing clock reading, retained across restarts.
2. **ftuA** — bounded, observable, control-side automatic recovery of a
   *cleanly drained* OUT lead-deadline fault (reason 55) in the independent
   4 ms/twelve-slot profile, keeping the admitted clients.

All timing and audio gates are unchanged: OUT lead 16 ms at every check, IN
≥ 2 ms checked / ≥ 1 ms returned, completion watchdog 64 ms, 4 ms × 12 geometry,
safety 1024/2560, replay 64 ms, PCM offset 3072, readiness cutoff 26 ms,
catch-up bound 10. The realtime HAL entry points are untouched. Ordinary `make`
still builds the 8 ms/4-slot baseline, where recovery is disabled
(`enabled:0`, every fault refused with `profile`).

## ftuE: last runtime fault (`ftu_live_runtime_fault`, `hal-check --runtime-fault`)

`struct ftu_runtime_snapshot { uint64_t words[46]; }`. Written only by the
epoch's worker (epochs are serialized by join), read through a bounded seqlock
(`valid:0` after four torn attempts; never blocks). Independent of ftuF, which
remains the lifetime latch of the *first failed USB completion* (in the incident
still epoch 1, `kIOReturnOverrun`).

Lifecycle:

- **Primary** words are published in the stream loop the moment `Owner::step`
  returns a non-`signal` error — before thread-policy restore, drain, cleanup or
  close. A panic in the stream loop publishes a `generic` record (reason 24).
- **Finish** (`finished:1`) adds drain/cleanup/close outcome, terminal phase and
  fault code after teardown, and before FAULT becomes visible.
- `Shared::reset`, a client Start and a recovery never clear it. A newer runtime
  fault replaces it; `overwritten` (saturating) counts replaced records and
  `records` counts all publications. A later healthy epoch therefore cannot hide
  the last fault.
- Startup/setup failures are not runtime records (see ftuI).

Fields guarded by a flag are **not measured** when the flag is clear. The JSON
prints them as `null`; zero is never substituted for a missing check time.

| Word | Name | Flag |
|---:|---|---|
| 0–2 | version (1), valid, present | |
| 3 | epoch | |
| 4 | reason (as ftuS `reason`; completion errors use their specific code) | |
| 5 | stage (below) | |
| 6 | flags | |
| 7–8 | direction (0 IN, 1 OUT), sequence | `1` direction |
| 9 | slot | `2` slot |
| 10 | scheduled bus frame (request start) | `4` scheduled |
| 11–12 | checked bus frame/host ticks (the final pre-submit check for stage 6; the failing check for other lead guards) | `8` checked |
| 13–14 | returned bus frame, host ticks after native submit returned | `16` returned |
| 15 | required lead (bus frames) of the failed check: 16 OUT, 2/1 IN, 64 watchdog | `1` |
| 16 | timeline base frame | `2048` base |
| 17–18 | prior step: `now` of the last step that returned Ok | `32` |
| 19–20 | prior progress: return clock of the last submission that passed every guard | `64` |
| 21 | owner steps this epoch (including the failing one) | |
| 22–27 | next IN/OUT, completed IN/OUT, pending IN/OUT at the decision | |
| 28–29 | captured, prepared (frames) | |
| 30–31 | timebase numer/denom | `1024` |
| 32 | minimum OUT return lead this epoch (includes a late after-return one) | `128` |
| 33 | native submit status (signed) | `256` |
| 34 | monotonic ms since handle creation (diagnostic/backoff only) | |
| 35 | finished | |
| 36–37 | drain failed, cleanup status (signed) | finished |
| 38 | close status (signed) | `512` + finished |
| 39–40 | terminal phase, terminal fault (signed) | finished |
| 41–42 | overwritten, records | |
| 43–45 | queue depth, block ms, replay ms | |

Stages (`stage_name` in JSON):

| Stage | Name | Guard in `Owner::step` |
|---:|---|---|
| 1 | `generic` | callback/control/clock error, native completion error, panic; no precise check clock. A failed post-submit clock read keeps direction/sequence/scheduled/checked, without a return clock. |
| 2 | `completion_watchdog` | an in-flight request older than start + 64 (reason 21) |
| 3 | `out_before_readiness` | OUT lead < 16 before PCM readiness (reason 55) |
| 4 | `before_packing` | clock after readiness: IN < 2 (reason 20) / OUT < 16 (55) |
| 5 | `after_packing` | clock after packing: IN < 2 / OUT < 16 |
| 6 | `after_return` | clock after native submit returned: IN < 1 / OUT < 16 |
| 7 | `submit_failed` | native submit returned an error (reason 26) |

The four OUT guards that all report reason 55 are now separable, and the failing
check's own frame and host ticks are recorded (returned fields at stage 6); a successful earlier return is
never reported in its place. The legacy timing/pump arrays still do not observe
independent callbacks; ftuE does not change that.

## ftuA: recovery state (`ftu_live_recovery`, `hal-check --recovery`)

`struct ftu_recovery_snapshot { uint64_t words[30]; }`, published under the
control lock to a seqlock; reads never take the lock. Counters persist across
epochs for the handle lifetime.

| Word | Name |
|---:|---|
| 0–2 | version (1), valid, enabled (independent profile) |
| 3 | state: 0 idle, 1 pending, 2 recovered, 3 refused, 4 exhausted, 5 cancelled, 6 healthy |
| 4 | refusal: 0 none, 1 quarantine, 2 reason, 3 cleanup/drain, 4 close, 5 fault code, 6 no record for this epoch, 7 unfinished, 8 attempt failed, 9 profile, 10 stage |
| 5–6 | consecutive attempts, budget |
| 7–13 | attempts, recovered, failed, refused, cancelled, exhausted, healthy resets |
| 14–15 | last handled fault epoch, epoch opened by the last successful recovery |
| 16 | last attempt status (signed) |
| 17–18 | pending since / due (ms) — valid with time_flags bit 1 |
| 19 | running since (ms) — valid with time_flags bit 2 |
| 20–23 | healthy ms, backoff ms for attempts 1–3 |
| 24–28 | last fault reason, binding generation, publication ms, active clients, retired clients |
| 29 | time_flags |

The plug-in adds `state_name`, `refusal_name`, `monitor_calls` and
`monitor_status` (its own last `ftu_live_recover` return).

## Policy

Recoverable only when **all** hold for the *current* epoch:

- independent profile; handle attached; caller's binding generation is current;
- at least one admitted client is still registered;
- phase FAULT (never QUARANTINED), the worker has returned and no admitted
  realtime call remains (`calls == 0`) — otherwise return 4 (draining), never a forced join;
- the ftuE record belongs to this epoch and is finished, terminal phase FAULT,
  drain succeeded, cleanup 0, close attempted and 0, shared and terminal fault 6
  (so thread-policy restore failure 13, postcondition 10 and panic 11 are refused);
- reason 55 at an OUT stage 3–6.

Every settled refusal is sticky for that epoch and visible. A newly qualified
clean failed restart is pending rather than refused. Quarantine is never
recovered. Setup failures without the new epoch’s qualified clean deadline
record, startup timeouts and other runtime reasons (including IN stale 20 and
watchdog 21) are not auto-retried.

Bounds (`recovery::POLICY`):

- budget **3** recoveries from consecutive eligible runtime faults; backoff
  before recovery 1/2/3 = **0 / 1000 / 4000 ms**, measured from the poll that
  first classified each fault. Each eligible fault receives one reopen attempt;
  these are not three attempts to overcome a failed startup;
- the consecutive count resets only when a recovered or client-started run has
  stayed up **30 s** — measured to the next fault's publication time, or observed
  while running — never on the RUNNING transition itself;
- budget exhausted → `exhausted` (-11) for that fault; no further attempts until
  the clients are gone and a client Start begins a new session;
- a recovery attempt that does not reach RUNNING within the existing 2 s start
  bound is counted `failed`. If its newer epoch has a verified finished clean
  reason-55 OUT deadline record, its worker joined and HAL admission drained,
  the remaining budget is used with the next backoff. Otherwise it is refused
  (`attempt_failed`) and never retried;
- driven at most once per monitor poll (500 ms) and by Starts; no busy loop.

Only a fresh client session, after all previous clients have stopped, restores
the consecutive budget. A repeated Start by a retained client or a new client
joining a faulted session uses the same policy and cannot bypass its budget.
Cumulative counters remain.

## Control semantics

All under the existing control mutex; `ftu_live_recover` uses `try_lock` and
returns 3 rather than waiting on a Start/Stop in progress.

| Event | Behaviour |
|---|---|
| Runtime fault, clients registered | Clients retained (not retired). Phase FAULT, actual DeviceIsRunning property false, ZTS/IO refused (-2). Notification ordering is described below. |
| Monitor poll, same bound unit | `ftu_live_recover(binding)`: policy above. On success: same Live, same pinned registry/location, **new epoch** (old worker joined, admitted IO drained first; old native context closed before the new open). |
| Start, retained or new ID, during a fault | Runs the same policy once; admitted (0) only if that recovers; otherwise -2 as before. |
| Stop of a retained client | Still a stop. The last one cancels a pending recovery (`cancelled`) and stops/joins as before; nothing is revived later. |
| Stop of an unknown ID | Removes nothing, cancels nothing for remaining clients. |
| Detach | Retires clients, cancels pending recovery, -6 afterwards. A retired ID is never revived (Start -9 until acknowledged). |
| Rebind | Requires no clients, as before; a stale binding returns -6. |
| Registry replacement without detach | The reopen is pinned to the bound registry and refused by the factory → `attempt_failed`, never retried. The plug-in detaches on the same poll before it would call recovery. |

Return codes of `ftu_live_recover`: 0 recovered, 1 nothing to do, 2 backoff,
3 lock busy, 4 worker finishing / IO draining, -1 null, -3 quarantined, -5 attempt did not reach running,
-6 detached/stale binding, -10 refused, -11 exhausted.

### Timeline and CoreAudio

The recovered session is a new epoch: new timeline base, new ZTS anchor, and
`GetZeroTimeStamp` reports a **changed seed**. Per the AudioServerPlugIn SDK, a
changed seed tells the HAL the device established a new timeline and that it
must resynchronize. No clock is faked, no samples from the old epoch are
replayed (timelines are epoch-offset), and nothing is spliced.

The plug-in monitor (`poll_presence`) attempts recovery **before** publishing a
stopped notification, only after confirming the same product/location/registry.
An immediate success preserves the notification state and changes the clock
seed. A worker just finishing, an admitted IO call draining, or a busy control
lock can defer the stop notification for at most two 500 ms polls. Property
reads and IO/timestamps still report the actual phase throughout; no clock or
running property is faked. Backoff, refusal, quarantine, removal or persistent
contention publishes the stopped state. A later successful recovery publishes
running again.

Recovery does not require another Start from the client. The offline IO
stand-in resumes against the new seed with its client registrations intact.
Actual CoreAudio/Spotify/GarageBand handling of the interruption still needs a
hardware test. There is no guarantee of an inaudible interruption: the prior
stream has already missed its deadline and stopped.

## Offline validation

Claude implemented the initial patch and performed a separate focused review.
Codex reviewed and corrected monitor notification ordering (including busy/drain
races), retired-client diagnostic publication, and the test harness. The old
monitor implementation fails the new notification-order regression.

- Full baseline `make test`: **214 Rust tests passed**, 3 existing ignored;
  native regression/mutation checks and **120 Python tests passed**.
- Linked production independent worker/native route: **36/36** cases passed
  normally, with Rust+C ThreadSanitizer, and with Rust AddressSanitizer+C UBSan.
- Targeted baseline ThreadSanitizer: **14/14** lifecycle/diagnostic cases passed.
- Both actual HAL boundaries pass C AddressSanitizer/UBSan, including the
  bounded notification ordering fixture; native discovery/open tests pass.
- Both Clippy profiles pass with warnings denied; `cargo fmt --check` passes.
- Qualification tools build; callback/continuity tests and the **60-test**
  analyzer suite pass. Both locally signed driver profiles built successfully.

The evidence retains the initial compile/test failures. One expected-clock
assertion confused the pre-submit check with the failed return. A startup fixture
missed the unchanged two-second bound while the large baseline stress suite ran
concurrently; isolated reruns passed. ThreadSanitizer exposed duplicate seed
counting on a transient read miss and unjoined finished workers in the intentional
quarantine fixtures. Those test defects were corrected without changing any
production start timeout, safety gate or quarantine behavior. The last test-only
TSan source difference is an equivalent OR-pattern-to-range Clippy edit; final
normal/ASan runs use the range form. Per-suite source hashes and logs are retained.

| Where | Runner | Covers |
|---|---|---|
| `runtime_fault::tests` | `cargo test -p ftu-stream`; linked runner (TSan/ASan) | layout, flags vs zero, overwrite/finish semantics, concurrent seqlock reads |
| `recovery::tests` | same | classification of every refusal, backoff, budget/exhaustion, healthy reset, cancel |
| `independent_live::tests::out_guards_record_their_exact_stage_and_clock` etc. | `test_live_completion.py` | the real `Owner` with linked C route: each OUT guard stage with exact frames/ticks/flags, IN stale stages, watchdog, clock/submit/callback generic stops, persistence through reset and clean teardown |
| `independent_live::tests::recovery::*` | `test_live_completion.py` | production worker via the test seam: one/two retained clients recovered without Start, continuous ZTS/IO thread across the fault and new seed, steady zero missing after recovery, Stop/unknown Stop/detach/rebind, IO drain wait, retained-client Start, refusals (IN stale, policy restore, registry replacement, close failure, cleanup failure), backoff/exhaustion/healthy reset, snapshot coherence under concurrent reads |
| `tests/hal-boundary.c` | `make test-hal`, `make test-hal-independent` | property list (12), ABI sizes, read-only/bad-size, null formatting, monitor gating (bound/removed/replacement), immediate-success ordering, bounded busy/drain grace, truthful phase reads and refusal/backoff notification |

Relevant commands (all offline):

```sh
cargo test --release --locked --offline -p ftu-stream
cargo test --release --locked --offline -p ftu-stream --no-default-features --features independent-4ms
python3 tools/ftu-probe/test_live_completion.py --quick
python3 tools/ftu-probe/test_live_completion.py --sanitizer thread --case recovery --case runtime_fault --case guards
python3 tools/ftu-probe/test_live_completion.py --sanitizer address --case recovery --case runtime_fault
make test-hal && make test-hal-independent
```

The test seam (`independent_live::Seam`) exists only under
`cfg(all(test, feature = "native-integration"))`. It substitutes the linked fake
route for the IOKit route, a virtual bus clock, and one owner-thread service
call per stream pass that delivers fake completions. Factory open, startup
initializer, Owner, guards, teardown, cleanup and close are the production code.

## Qualification remains strict

The tone/silence, two-client and loopback tools now verify the stream epoch
before Start, throughout the run and after Stop. A trial already starting from
a faulted/starting/stopping phase is rejected. A stop/recovery entirely
between polls still fails the normal audio trial even if the latest phase and
per-epoch counters look healthy. Recovery therefore cannot hide a glitch or
make a discontinuous run count as zero-loss qualification. No old latency
reference, timing guard, audio-loss gate or shutdown rule was relaxed.

## Remaining hardware questions

- How real CoreAudio clients react to refused IO/timestamps during recovery and
  the subsequent seed change, including whether they keep their registrations.
- Whether this interface accepts immediate reopen after a clean reason-55 stop.
- Whether the 0/1000/4000 ms policy and 30 s healthy window suit real use.
- The delay that caused the original 28 October fault: the earlier build did
  not record it. The new diagnostic makes a recurrence attributable to its guard
  and clock readings; it does not yet establish the cause.

Startup failures, USB removal/replacement, watchdog/IN failures, failed cleanup
or close, and quarantine are not retry targets. A recovery launch that itself
fails is recorded and stays refused; broadening startup retries needs separate
ownership/initializer evidence. The three-attempt budget applies to repeated
eligible runtime faults after successful reopen. Endless retries would hide a
persistent failure and are deliberately excluded.

## Frozen candidate and next check

Candidate `dev-369d1059e326aa6d` keeps the installed experimental 4 ms/twelve-slot
geometry. Ordinary `make install` still builds the 8 ms/four-slot baseline,
where this automatic recovery is disabled. Use this frozen candidate for the
controlled test:

```bash
/bin/bash /Users/bruno/development/projects/bruno/AudioInterfaces/FastTrackUltra-AppleSilicon/build/validation/runtime-recovery-20261028/install.sh
```

Exact rollback to the driver installed at the start of this work:

```bash
/bin/bash /Users/bruno/development/projects/bruno/AudioInterfaces/FastTrackUltra-AppleSilicon/build/validation/runtime-recovery-20261028/rollback.sh
```

Both commands support `--check` to validate without installation. Installing
requires an administrator password and restarts CoreAudio; this offline work
has not run either installation. No stream, USB reset or audio-selection change
was made. The source changes remain uncommitted.

Candidate binary SHA-256: `0c66982319bac1b9d26a9a5458454f20aa3d4c91f75976fa6dcec75fecc13675`.
Rollback `dev-0d800cbf6bdead51` SHA-256:
`a70672c39983242e1446c1dbe0dd2728a91a7535f27aa39453eea9212397f8e5`.

Source snapshot, review, all initial failures and successful results:
`build/validation/runtime-recovery-20261028/`. Candidate, exact rollback and
updated inspectors: `build/candidates/runtime-recovery-20261028/`.

After installation: verify the loaded identity, perform the same silence
preflight and short loopback, then exercise app stop/start and longer playback.
Confirm the current cable routing before loopback. Query `hal-check --runtime-fault`
and `hal-check --recovery` if a fault occurs; these options only read metadata.
Do not count a recovered session as an uninterrupted audio pass. There is no
production fault-injection switch; real automatic resumption remains unqualified
until observed safely with actual clients. Physical reconnect and the fresh
64-frame latency comparison remain subsequent gates.

## Disconnect follow-up (30 October)

[The disconnect retirement candidate](DISCONNECT-RECOVERY.md) distinguishes
safely retired removed services (Fault 16) from unsafe ownership quarantine.
Neither is eligible for same-binding automatic recovery. Terminal quarantine
now updates ftuA even after detach leaves no active clients; historical counters
are retained but the current state is refused/quarantine. The new ftuQ record
shows exact native shutdown outcomes separately from the terminal ftuE/ftuS.
Hardware reconnect and CoreAudio availability validation remain pending.
