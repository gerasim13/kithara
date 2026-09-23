# kithara-play-tests

Playback, mixing, seeking, buffering, and player lifecycle tests live in
[`tests`](tests). Regular, heavy, stress, device, and network contracts use
separate binaries so each execution surface keeps an independent cached artifact.

Warp tempo/pitch and rate-response contracts live in `kithara-warp-tests`.
Synchronization contracts across Host, Player, and Queue live in
`kithara-sync-tests`. Functional assertions determine ownership; the test driver
does not.
