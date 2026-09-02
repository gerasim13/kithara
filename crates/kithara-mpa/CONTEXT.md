# kithara-mpa

## Why this fork exists

Over a network source, `MediaSourceStream` (MSS) answers a read with
`WouldBlock` / `Interrupted` when the bytes are not downloaded yet. Upstream's
`MpaReader` consumes bytes from its read-ahead ring *before* it knows the read
can complete, so a transient error leaves the cursor advanced with no packet
emitted.

`kithara-decode` recovers that for every other reader by re-seeking to a
retained timestamp (`SymphoniaDemuxer::reseek_to_resume`). For MPEG audio that
recovery is wrong: the retry resyncs to the *next* frame header while
`next_packet_ts` has already counted the part-consumed frame, so every later
timestamp shifts by one frame.

The fix has to live where frame boundaries are known, which is inside the
reader. `MpaReader`'s fields are private upstream, so a wrapper cannot do it.

## The contract

Every frame read is a transaction:

- checkpoint the MSS cursor and `ensure_seekback_buffer(MAX_MPEG_FRAME_SIZE)`
  before the read;
- on `Interrupted` / `WouldBlock`, restore the checkpoint and return the
  original error, so the caller retries from the same frame boundary;
- a rollback that does not land exactly on the checkpoint is a terminal
  `decode_error`, never a silent approximation.

This holds on two paths: `next_packet` (via `read_mpeg_frame`) and the
frame-scan loop inside `seek` (via `roll_back_transient`). The checkpoint is
per frame, so discarded Xing/Info/VBRI frames commit independently and
`next_packet_ts` advances only after a complete frame.

Pending from inside `MpaReader::seek` is not made resumable by this contract;
seek transactionality remains a separate concern.

`kithara-decode` pairs with this by *not* layering timestamp recovery over the
rollback — see `reseek_to_resume` there, keyed on `FORMAT_ID_MP1/MP2/MP3`. The two
mechanisms are exclusive: running both discards the exact rollback.

## Relationship to upstream

Base: `symphonia-bundle-mp3` 0.6.0 (byte-identical to git rev
`980bf5830a90e069fd64641d9c38f067ab772a24`). The workspace resolves the rest of
the Symphonia stack to 0.6.0 as well; 0.6.1 changed this demuxer, so bumping the
stack and rebasing this fork are one job, not two.

Files are vendored verbatim except for:

- the rollback delta above;
- `log` -> `tracing`;
- comment cleanup required by the workspace lint policy; the MPL notices stay
  intact as legal comment headers;
- the workspace lint gate, which this crate is a full member of. That forced
  `pub` -> `pub(crate)` on the items no longer re-exported, `as` casts replaced
  by `From` / `try_from` / `num-traits`, the frame-scan loop lifted out of `seek`
  into `scan_to`, `ChannelMode::count` expressed as `From<ChannelMode> for
  usize`, module-level constants grouped behind `TagIds` / `FormatInfos` /
  `BitRates`, and Xing/Info/VBRI tag parsing moved out of `demuxer.rs` into
  `tags.rs` to stay under the 1000-line file ratchet. All are
  behaviour-preserving; none may be reverted to ease a rebase, because the
  alternative is a lint suppression.

`common.rs` and `header.rs` are copied only because they are `pub(crate)`
upstream and the demuxer needs them.

To rebase onto a new upstream release, diff `src/` against that release's
`symphonia-bundle-mp3/src/` — `demuxer.rs` plus `tags.rs` against upstream's
`demuxer.rs` — and keep only the rollback delta. The lint-forced changes above
are already drift; do not add more.

The delta is upstreamable and should be offered to
<https://github.com/pdeljanov/Symphonia>. If it lands, this crate is deleted
and `kithara-decode` registers the upstream reader again.

## License

MPL-2.0, inherited from Symphonia. The workspace is MIT OR Apache-2.0; this
crate is the one exception, which is why the fork is a separate crate rather
than a module inside `kithara-decode`.
