# Combined timing and Surge-guard check

This is a source-bound **debug functional check**, separate from the historical
full matrix and from pending optimized qualification. The original generated
summary is retained byte-for-byte (SHA256
`57300eb4cac2e40eb81d7db9cb4228c475f11d7a12bfee82c63c1cf0b867d0e7`).
It binds123 production source files to4fdfbc2, the copied harness to d38e6a2b,
and the helper to9dd18d74. Full identities and raw non-audio receipt hashes are
inside the summary. Task-local paths, binaries, state blobs and audio are absent.
The raw run records remain in the development QA archive; this small publication
is a summary and does not distribute a complete new reproduction harness.

All four default2048 ordinary/routed and fixed/changing cases pass real native
FX32 latency replan, stopped Retry/new revision/epoch, exact PDC/events and delivery
(375 completed,357 exact plugin quanta,18 startup per active endpoint). Fresh Surge
state restore before the first Process immediately sounds its first note and keeps
controller/component volume. The smaller128 matched control has two passes and
two FX DeadlineMiss failures. No capture overflow occurred.

Delivery PASS is **not timing qualification**: changing callbacks at2048 still had
14 callback-core interval overruns in each case. The passing128 changing cases had
18/24 overruns. These measure the external debug wrapper plus production callback,
not isolated DSP or hardware. Capture cost and outer scheduling are reported
separately; optimized, device and native-editor continuity remain unqualified.
