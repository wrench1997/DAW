# MIDI routing panic-wait diagnostics

The Windows preview run 39031653598 on source 98b6 reported the deliberate
source-process panic, then expired the MIDI fixture's two-second worker wait.
The log does not establish whether ordinary panic-hook/backtrace work, unwind,
catch, output publication, or a different worker was pending at the deadline.
This diagnostic-only change does not fix or claim a root cause for that failure.

The original completion predicate, deadline, yield loop, held-note cleanup,
no-resurrection and bounded-silence checks remain. A timeout now records the
waiting control index, submitted target, failure mode, frame context, all control
stats and named source/sink/insert snapshots. Snapshots include submitted,
completed, old-input/output drops, output overflows, stopped, input consumed,
exactly-once operations completed and worker busy. These are observations, not
additional success conditions. In particular, adding old-output drops to completed
would double-count callback discards of already-published outputs.

Mode 3 arms a run-owned trace while the callback driver is paused, before changing
the failure flag. The source adapter supplies its actual next sequence and epoch.
The trace observes panic request, unwind beginning, catch return and successful
publication of that exact output. The worker-local trace cannot be advanced by
another worker or a different epoch/sequence; a focused regression also exercises
a sequence greater than one. All runtime trace hooks are test-only. No global
panic hook is replaced, output consumed, backend polled or timeout increased.
A missing unwind mark after panic request is consistent with work before unwind
(including an ordinary hook); it is not by itself proof of a particular hook or
backtrace subsystem. Counter snapshots are best-effort and do not claim a coherent
multi-worker instant. Original Windows evidence remains unresolved until a new
failure supplies the phase and worker observations.
