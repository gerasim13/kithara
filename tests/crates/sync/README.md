# kithara-sync-tests

Functional synchronization acceptance tests live in [`tests`](tests): operation
ordering and tempo rides, runtime PCM synchronization oracles, listening
artifacts, and synchronization-fixture validation. The tests compose Host,
Player, and Queue through the existing integration support; their assertions
belong to the [Sync contract](https://github.com/zvuk/kithara/wiki/kithara-sync).

Run the suite through `just test run --lane=sync`.
Plain playback, source-rate conversion, and seek continuity remain in
`kithara-play-tests`.
