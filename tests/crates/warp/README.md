# kithara-warp-tests

Warp behavior tests: source/output projection, tempo and speed response,
keylock, varispeed, and PCM continuity. Host, Player and Queue are test drivers
where the contract must be checked through presented PCM.

```sh
just test run --lane=warp
```

The `warp` binary exercises public DSP contracts. `warp_playback` contains the
rate-response integration suite, including the unchanged 441-frame budgets.
Private renderer state tests remain in `kithara-warp`.

For DSP-only iteration without the playback helper dependency graph:

```sh
just test run -p kithara-warp-tests --test warp --net-backend=none --no-default-features --features stretch-signalsmith
```

The optional `playback` feature controls the integration environment, not test
ownership. It is enabled by default, so regular acceptance discovers both
binaries. Existing fixtures and assertions are reused. No build-time speedup
has been measured.

See the [Warp contract](https://github.com/zvuk/kithara/wiki/kithara-warp).
