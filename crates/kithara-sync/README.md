# kithara-sync

Pure synchronization mathematics: sided entry placement, grid coverage, phase
error, bounded speed correction, and continuous piecewise-constant tempo
trajectories. It owns no player, renderer, group topology, or receipt custody.

## Usage

Construct a `TempoTrajectory` from a `TempoStep`, meter, and output sample rate.
Use `entry(&trajectory, &grid, position, Bound::AtOrAfter(frame))` or
`Bound::AtOrBefore(frame)` to find the nearest in-phase entry while preserving
the media position, expressed as `std::time::Duration`. Use `covers` before
`phase_error`, and `speed` for the host-to-track tempo ratio.

See the [Sync contract](https://github.com/zvuk/kithara/wiki/kithara-sync).
